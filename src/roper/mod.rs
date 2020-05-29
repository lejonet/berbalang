use std::sync::Arc;

use byteorder::{BigEndian, ByteOrder, LittleEndian};
use indexmap::map::IndexMap;
use rand::seq::IteratorRandom;
use rand::{thread_rng, Rng};
use serde_derive::Deserialize;
use toml;
use unicorn::{Cpu, CpuARM, CpuARM64, CpuM68K, CpuMIPS, CpuSPARC, CpuX86};

use crate::configure::Configure;
use crate::emulator::executor;
use crate::util::architecture::Endian::Little;
/// This is where the ROP-evolution-specific code lives.
use crate::{
    emulator::executor::{Hatchery, HatcheryParams, Register},
    emulator::loader,
    evolution::{Epoch, Genome, Phenome},
    fitness::FitnessScore,
    util::architecture::{endian, word_size, Endian},
};

fn default_min_init_len() -> usize {
    1
}
fn default_max_init_len() -> usize {
    64
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub binary_file: String,
    pub gadget_file: String,
    #[serde(default = "Default::default")]
    pub soup: Vec<u64>,
    pub arch: unicorn::Arch,
    pub mode: unicorn::Mode,
    pub population_size: usize,
    pub num_offspring: usize,
    pub mutation_rate: f32,
    pub tournament_size: usize,
    #[serde(default = "default_min_init_len")]
    pub min_init_len: usize,
    #[serde(default = "default_max_init_len")]
    pub max_init_len: usize,
    pub max_final_len: usize,
    pub observer_window_size: usize,
    pub data_file: Option<String>,
    pub register_pattern: Option<RegisterPatternConfig>,
}

impl Configure for Config {
    fn assert_invariants(&self) {
        unimplemented!()
    }

    fn mutation_rate(&self) -> f32 {
        self.mutation_rate
    }

    fn tournament_size(&self) -> usize {
        self.tournament_size
    }

    fn population_size(&self) -> usize {
        self.population_size
    }

    fn observer_window_size(&self) -> usize {
        self.observer_window_size
    }

    fn num_offspring(&self) -> usize {
        self.num_offspring
    }

    fn max_length(&self) -> usize {
        self.max_final_len
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct RegisterPatternConfig(pub IndexMap<String, u64>);

#[derive(Debug)]
pub struct RegisterPattern<C: 'static + Cpu<'static>>(pub IndexMap<Register<C>, u64>);

macro_rules! register_pattern_converter {
    ($cpu:ty) => {
        impl From<RegisterPatternConfig> for RegisterPattern<$cpu> {
            fn from(rp: RegisterPatternConfig) -> Self {
                let mut map: IndexMap<Register<$cpu>, u64> = IndexMap::new();
                for (reg, num) in rp.0.iter() {
                    let r: Register<$cpu> =
                        toml::from_str(&reg).expect("Failed to parse register pattern");
                    map.insert(r, *num);
                }
                Self(map)
            }
        }
    };
}

register_pattern_converter!(CpuX86<'static>);
register_pattern_converter!(CpuARM<'static>);
register_pattern_converter!(CpuARM64<'static>);
register_pattern_converter!(CpuMIPS<'static>);
register_pattern_converter!(CpuSPARC<'static>);
register_pattern_converter!(CpuM68K<'static>);

fn bit(n: u64, bit: usize) -> bool {
    (n >> (bit as u64 % 64)) & 1 == 1
}

#[derive(Debug)]
pub struct Genotype {
    pub crossover_mask: u64,
    pub chromosome: Vec<u64>,
    pub tag: u64,
    pub name: String,
    pub parents: Vec<String>,
}

impl Genome<Config> for Genotype {
    fn random(params: &Config) -> Self {
        let mut rng = rand::thread_rng();
        let length = rng.gen_range(params.min_init_len, params.max_init_len);
        let chromosome = params
            .soup
            .iter()
            .choose_multiple(&mut rng, length)
            .into_iter()
            .copied()
            .collect::<Vec<u64>>();
        let name = crate::util::name::random(4);
        let crossover_mask = rng.gen::<u64>();
        let tag = rng.gen::<u64>();
        Self {
            crossover_mask,
            chromosome,
            tag,
            name,
            parents: vec![],
        }
    }

    fn crossover(&self, mate: &Self, _params: &Config) -> Vec<Self>
    where
        Self: Sized,
    {
        // TODO experiment with different mask combiners
        let mask = self.crossover_mask ^ mate.crossover_mask;
        let cross = |mother: &Vec<u64>, father: &Vec<u64>| -> Self {
            let mut chromosome = mother.clone();
            for i in 0..chromosome.len() {
                if bit(mask, i) {
                    chromosome[i] = father[i % father.len()];
                }
            }
            Self {
                crossover_mask: mask,
                chromosome,
                tag: rand::random::<u64>(),
                name: "".to_string(),
                parents: vec![self.name.clone(), mate.name.clone()],
            }
        };

        vec![
            cross(&self.chromosome, &mate.chromosome),
            cross(&mate.chromosome, &self.chromosome),
        ]
    }

    fn mutate(&mut self, params: &Config) {
        let mut rng = thread_rng();
        let i = rng.gen_range(0, self.chromosome.len());

        match rng.gen_range(0, 5) {
            0 => {
                // Dereference mutation
                let memory = loader::get_static_memory_image();
                if let Some(bytes) = memory.try_dereference(self.chromosome[i]) {
                    if bytes.len() > 8 {
                        let endian = endian(params.arch, params.mode);
                        let word_size = word_size(params.arch, params.mode);
                        let word = match (endian, word_size) {
                            (Endian::Little, 8) => LittleEndian::read_u64(bytes),
                            (Endian::Big, 8) => BigEndian::read_u64(bytes),
                            (Endian::Little, 4) => LittleEndian::read_u32(bytes) as u64,
                            (Endian::Big, 4) => BigEndian::read_u32(bytes) as u64,
                            (Endian::Little, 2) => LittleEndian::read_u16(bytes) as u64,
                            (Endian::Big, 2) => BigEndian::read_u16(bytes) as u64,
                            (_, _) => unimplemented!("I think we've covered the bases"),
                        };
                        self.chromosome[i] = word;
                    }
                }
            }
            1 => {
                // Indirection mutation
                let memory = loader::get_static_memory_image();
                let word_size = word_size(params.arch, params.mode);
                let mut bytes = vec![0; word_size];
                let word = self.chromosome[i];
                match (endian(params.arch, params.mode), word_size) {
                    (Endian::Little, 8) => LittleEndian::write_u64(&mut bytes, word),
                    (Endian::Big, 8) => BigEndian::write_u64(&mut bytes, word),
                    (Endian::Little, 4) => LittleEndian::write_u32(&mut bytes, word as u32),
                    (Endian::Big, 4) => BigEndian::write_u32(&mut bytes, word as u32),
                    (Endian::Little, 2) => LittleEndian::write_u16(&mut bytes, word as u16),
                    (Endian::Big, 2) => BigEndian::write_u16(&mut bytes, word as u16),
                    (_, _) => unimplemented!("I think we've covered the bases"),
                }
                if let Some(address) = memory.seek_from_random_address(&bytes) {
                    self.chromosome[i] = address;
                }
            }
            3 => {
                self.chromosome[i].wrapping_add(rng.gen_range(0, 0x100));
            }
            4 => {
                self.chromosome[i].wrapping_sub(rng.gen_range(0, 0x100));
            }
            _ => unimplemented!("out of range"),
        }
    }
}
