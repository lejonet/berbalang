#[macro_export]
macro_rules! impl_dominance_ord_for_phenome {
    ($phenome:ty, $ord:ident) => {
        #[derive(Clone, Debug, Copy)]
        pub struct $ord;

        impl ::non_dominated_sort::DominanceOrd<$phenome> for $ord {
            fn dominance_ord(&self, a: &$phenome, b: &$phenome) -> std::cmp::Ordering {
                a.fitness()
                    .partial_cmp(&b.fitness())
                    .unwrap_or(std::cmp::Ordering::Equal)
            }
        }

        impl<'a> ::non_dominated_sort::DominanceOrd<&'a $phenome> for $ord {}
    };
}

#[macro_export]
macro_rules! hashmap {
    ($($key:expr => $val:expr$(,)?)*) => {
        {
            let mut map = ::hashbrown::HashMap::new();
            $(
                let _ = map.insert($key.into(), $val);
            )*
            map
        }
    }

}

#[macro_export]
macro_rules! pareto {
    ($($key:expr => $val:expr, $(,)?)*) => {
        Pareto(hashmap!{$( $key => $val, )*})
    }
}

#[macro_export]
macro_rules! lexical {
    ($($v:expr $(,)?)*) => {
        vec![$( $v, )*]
    }
}
