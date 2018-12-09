use crate::firm::Program;
use libfirm_rs::bindings;
use std::ffi::CString;

mod constant_folding;
mod fixpoint;
mod unreachable_code_elimination;

pub use self::fixpoint::Fixpoint;

#[derive(
    strum_macros::EnumString,
    serde_derive::Deserialize,
    serde_derive::Serialize,
    Debug,
    Copy,
    Clone,
    Display,
)]
pub enum OptimizationKind {
    AlgebraicSimplification,
    ConstantFolding,
    UnreachableCodeElimination,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GlobalOptimizationFlag {
    DumpYcomp,
}

#[derive(Clone, Debug)]
pub struct Optimization {
    pub kind: OptimizationKind,
    pub flags: Vec<GlobalOptimizationFlag>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum OptimizationResult {
    Unchanged,
    Changed,
}

pub struct OptimizationResultCollector {
    results: Vec<OptimizationResult>,
}

impl OptimizationResultCollector {
    pub fn new() -> OptimizationResultCollector {
        OptimizationResultCollector {
            results: Vec::new(),
        }
    }

    pub fn push(&mut self, res: OptimizationResult) {
        self.results.push(res);
    }

    pub fn result(&self) -> OptimizationResult {
        if self
            .results
            .iter()
            .any(|x| *x == OptimizationResult::Changed)
        {
            OptimizationResult::Changed
        } else {
            OptimizationResult::Unchanged
        }
    }
}

/// run a list of optimizations on the given program
pub fn run_all(program: &Program<'_, '_>, optimizations: &[Optimization]) {
    if let Err(_) = std::env::var("COMPRAKT_OPTIMIZATION_NO_FIXPOINT") {
        let fp = Fixpoint::new(optimizations);
        fp.run(program);
        return;
    }

    for (i, optimization) in optimizations.iter().enumerate() {
        log::debug!("Running optimization #{}: {:?}", i, optimization.kind);
        optimization.run(program);
        if optimization
            .flags
            .iter()
            .any(|f| *f == GlobalOptimizationFlag::DumpYcomp)
        {
            unsafe {
                let suffix = CString::new(format!("{:?}", optimization.kind)).unwrap();
                log::debug!("Dumping graph with suffix {:?}", suffix);
                bindings::dump_all_ir_graphs(suffix.as_ptr());
            }
        }
        log::debug!("Finished optimization #{}: {:?}", i, optimization.kind);
    }
}

impl Optimization {
    fn run(&self, program: &Program<'_, '_>) -> OptimizationResult {
        use self::OptimizationKind::*;
        match self.kind {
            AlgebraicSimplification => unimplemented!(),
            ConstantFolding => constant_folding::run(program),
            UnreachableCodeElimination => unreachable_code_elimination::run(program),
        }
    }
}
