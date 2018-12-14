use super::{OptimizationResult, OptimizationResultCollector};
use crate::firm::Program;
use libfirm_rs::{
    bindings,
    graph::Graph,
    nodes::NodeTrait,
    nodes_gen::{Node, ProjKind},
};

struct UnreachableCodeElimination {
    graph: Graph,
}

pub fn run(program: &Program<'_, '_>) -> OptimizationResult {
    let mut collector = OptimizationResultCollector::new();
    for class in program.classes.values() {
        for method in class.borrow().methods.values() {
            if let Some(graph) = method.borrow().graph {
                log::debug!("Graph for Method: {:?}", method.borrow().entity.name());
                collector.push(UnreachableCodeElimination::new(graph.into()).run());
            }
        }
    }
    collector.result()
}

impl UnreachableCodeElimination {
    fn new(graph: Graph) -> Self {
        Self { graph }
    }

    fn run(&mut self) -> OptimizationResult {
        unsafe {
            bindings::assure_irg_outs(self.graph.into());
        }

        let mut replacements = Vec::new();
        let mut dangling_nontarget_blocks = Vec::new();
        self.graph.walk_topological(|node| {
            if let Node::Cond(cond) = node {
                if let Node::Const(c) = cond.selector() {
                    let tarval = c.tarval();

                    let used_proj = node
                        .out_nodes()
                        .filter_map(|out_node| match out_node {
                            Node::Proj(proj, ProjKind::Cond_True(_))
                                if tarval.is_bool_val(true) =>
                            {
                                Some(proj)
                            }
                            Node::Proj(proj, ProjKind::Cond_False(_))
                                if !tarval.is_bool_val(true) =>
                            {
                                Some(proj)
                            }
                            _ => None,
                        })
                        .next()
                        .expect("Cond node must have Cond_{True,False} projection");

                    let unused_proj = node
                        .out_nodes()
                        .filter_map(|out_node| match out_node {
                            Node::Proj(proj, ProjKind::Cond_True(_))
                                if !tarval.is_bool_val(true) =>
                            {
                                Some(proj)
                            }
                            Node::Proj(proj, ProjKind::Cond_False(_))
                                if tarval.is_bool_val(true) =>
                            {
                                Some(proj)
                            }
                            _ => None,
                        })
                        .next()
                        .expect("Cond node must have Cond_{True,False} projection");

                    // STEP
                    // Enqueue the replacement of Cond with a Jmp to used_proj
                    let target_block = used_proj
                        .out_nodes()
                        .next()
                        .expect("Proj node must have successor");
                    let target_block = if let Node::Block(block) = target_block {
                        block
                    } else {
                        unreachable!("Target of a Proj must be a Block")
                    };

                    let jmp = cond.block().new_jmp();
                    replacements.push((used_proj, jmp, unused_proj, target_block));
                    log::debug!("Replace {:?} with {:?} to {:?}", node, jmp, target_block);

                    // STEP
                    // If the unused_proj is the sole predecessor of its successor,
                    // mark the successor, eliminate it.
                    // One would think this happens automatically, but it doesn't:
                    // The successor block (if(f?) it contains a Jmp), is kept alive
                    // somehow, causing be_lower_for_target to fail
                    let nontarget_block = unused_proj
                        .out_nodes()
                        .next()
                        .expect("Proj node must have successor");
                    let nontarget_block = if let Node::Block(block) = nontarget_block {
                        block
                    } else {
                        unreachable!("Target of a Proj must be a Block")
                    };
                    if nontarget_block.num_cfgpreds() <= 1 {
                        log::debug!("Mark nontarget block {:?} as dangling", nontarget_block);
                        dangling_nontarget_blocks.push(nontarget_block);
                    }
                }
            }
        });

        for (used_proj, jmp, unused_proj, target_block) in &replacements {
            // Now we replace the always-taken path with an unconditional jump ...
            log::debug!("Exchange unused proj {:?} with jmp {:?}", unused_proj, jmp,);
            Graph::exchange(used_proj, jmp);

            // ... and mark the never-taken as "bad", so it will be later removed
            log::debug!("Mark unused proj {:?} as bad", unused_proj);
            self.graph.mark_as_bad(unused_proj);

            // We need this because if we have a while(true) loop, the code will be
            // unreachable (libfirm-edge wise) from the end block (because the end block is
            // never reached control-flow wise), but libfirm needs to find the
            // loop (and it starts searching from the end block)
            target_block.keep_alive();
        }

        for nontarget_block in &dangling_nontarget_blocks {
            let outs = nontarget_block.out_nodes().collect::<Vec<_>>();
            for (i, child) in outs.iter().enumerate() {
                log::debug!("Mark Nontarget block child #{} {:?}", i, child);
                self.graph.mark_as_bad(child);
            }
        }

        // Now we actually remove the unreachable code
        self.graph.remove_bads();

        if replacements.len() + dangling_nontarget_blocks.len() > 0 {
            OptimizationResult::Changed
        } else {
            OptimizationResult::Unchanged
        }
    }
}