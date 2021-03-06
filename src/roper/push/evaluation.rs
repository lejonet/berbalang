use std::convert::TryInto;
use std::sync::Arc;

use unicorn::Cpu;

use crate::configure::Config;
use crate::emulator::hatchery::Hatchery;
use crate::emulator::profiler::{HasProfile, Profile};
use crate::emulator::register_pattern::{Register, UnicornRegisterState};
use crate::evolution::{Genome, Phenome};
use crate::fitness::Weighted;
use crate::ontogenesis::{Develop, FitnessFn};
use crate::roper::push;
use crate::roper::push::{Creature, MachineState};
use crate::roper::Sketches;
use crate::util;

pub struct Evaluator<C: Cpu<'static> + 'static> {
    config: Arc<Config>,
    hatchery: Hatchery<C, push::Creature>,
    sketches: Sketches,
    fitness_fn: Box<FitnessFn<push::Creature, Sketches, Config>>,
}

impl<C: 'static + Cpu<'static>> Evaluator<C> {
    pub fn spawn(config: &Config, fitness_fn: FitnessFn<Creature, Sketches, Config>) -> Self {
        let mut config = config.clone();
        config.roper.parse_register_pattern();
        let hatch_config = Arc::new(config.roper.clone());
        let register_pattern = config.roper.register_pattern();
        let output_registers: Vec<Register<C>> = {
            let mut out_reg: Vec<Register<C>> = config
                .roper
                .output_registers
                .iter()
                .map(|s| s.parse().ok().expect("Failed to parse output register"))
                .collect::<Vec<_>>();
            if let Some(pat) = register_pattern {
                let arch_specific_pat: UnicornRegisterState<C> =
                    pat.try_into().expect("Failed to parse register pattern");
                let regs_in_pat = arch_specific_pat.0.keys().cloned().collect::<Vec<_>>();
                out_reg.extend_from_slice(&regs_in_pat);
                out_reg.dedup();
                out_reg
            } else {
                out_reg
                //todo!("implement a conversion method from problem sets to register maps");
            }
        };
        let inputs = if config.roper.randomize_registers {
            vec![util::architecture::random_register_state::<u64, C>(
                &output_registers,
                config.random_seed,
            )]
        } else {
            vec![util::architecture::constant_register_state::<C>(
                &output_registers,
                1_u64,
            )]
        };
        let hatchery: Hatchery<C, Creature> = Hatchery::new(
            hatch_config,
            Arc::new(inputs),
            Arc::new(output_registers),
            None,
        );

        let sketches = Sketches::new(&config);
        Self {
            config: Arc::new(config),
            hatchery,
            sketches,
            fitness_fn: Box::new(fitness_fn),
        }
    }
}

impl<C: 'static + Cpu<'static>> Develop<push::Creature> for Evaluator<C> {
    fn develop(&self, mut creature: push::Creature) -> push::Creature {
        let args = vec![];

        // TODO: evaluate per-problem.
        // What has to be done to make this possible?
        // - the payload needs to be changed from an Option container to a HashMap, associating each
        //   problem with its own payload.
        // - we then need to reconsider how profiles are collated and reported upon. They were originally
        //   designed with multiple problems in mind, but we haven't really tested this out, yet.

        let mut machine = MachineState::default();
        if creature.payload.is_none() {
            let payload = machine.exec(creature.chromosome(), &args, self.config.push_vm.max_steps);
            creature.payload = Some(payload);
        }
        // a maximal (very bad) fitness value should be assigned if the payload is empty
        // so we can skip evaluation in this case.

        if creature.profile.is_none() {
            let empty_payload = creature.payload.as_ref().map(Vec::is_empty);
            if let Some(false) = empty_payload {
                let (mut creature, profile) = self
                    .hatchery
                    .execute(creature)
                    .expect("Failed to evaluate creature");
                creature.profile = Some(profile);
                creature
            } else {
                // this will mark the profile as non-executable
                // log::warn!("Creature with empty payload. declaring it a complete failure");
                let profile = Profile::default();
                debug_assert!(!profile.executable);
                creature.profile = Some(profile);
                creature
            }
        } else {
            creature
        }
    }

    fn apply_fitness_function(&mut self, mut creature: push::Creature) -> push::Creature {
        let profile = creature
            .profile()
            .expect("Attempted to apply fitness function to undeveloped creature");
        if !profile.executable {
            let mut fitness = Weighted::new(&self.config.fitness.weighting);
            fitness.declare_failure();
            creature.set_fitness(fitness);
            creature
        } else {
            (self.fitness_fn)(creature, &mut self.sketches, self.config.clone())
        }
    }

    fn development_pipeline<I: 'static + Iterator<Item = push::Creature> + Send>(
        &self,
        inbound: I,
    ) -> Vec<push::Creature> {
        inbound
            .into_iter()
            .map(|c| self.develop(c))
            .collect::<Vec<push::Creature>>()
    }
}
