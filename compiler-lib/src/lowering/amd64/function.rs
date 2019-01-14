use super::{
    linear_scan, live_variable_analysis::LiveVariableAnalysis, register::RegisterAllocator, var_id,
    Amd64Reg, CallingConv, Instruction, MoveOperand, Operand, VarId,
};
use crate::lowering::{lir, lir_allocator::Ptr};
use interval::{ops::Range, Interval};
use libfirm_rs::Tarval;
use std::collections::{BTreeSet, HashMap};

macro_rules! save_regs {
    ([$($reg:ident),*], $instr_kind:ident, $save_instrs:expr, $restore_instrs:expr) => {{
        $(
            $save_instrs.push($instr_kind::Pushq {
                src: Operand::Reg(Amd64Reg::$reg),
            });
            $restore_instrs.push($instr_kind::Popq {
                dst: Operand::Reg(Amd64Reg::$reg),
            });
        )*
    }};
}

type Label = String;

#[derive(Debug, Copy, Clone)]
pub(super) enum FnInstruction {
    Movq { src: Operand, dst: Operand },
    Pushq { src: Operand },
    Popq { dst: Operand },
    Addq { src: Tarval, dst: Operand },
}

#[derive(Debug, Default, Clone)]
pub(super) struct FunctionCall {
    /// Save the own arguments
    arg_save: Vec<FnInstruction>,
    /// Put the parameters where they belong
    pub(super) setup: Vec<FnInstruction>,
    /// Call label
    label: Label,
    /// If a result is produced, move it in the register
    pub(super) move_res: Option<FnInstruction>,
    /// If arguments were put on the stack, reset the stack pointer
    recover: Option<FnInstruction>,
    /// Get the own arguments back
    arg_recover: Vec<FnInstruction>,
}

impl FunctionCall {
    pub(super) fn new(cconv: CallingConv, call_instr: lir::Instruction) -> Self {
        let mut call = Self::default();

        match cconv {
            CallingConv::X86_64 => call.setup_x86_64_cconv(call_instr),
            CallingConv::Stack => call.setup_stack_cconv(call_instr),
        }

        call
    }

    fn setup_x86_64_cconv(&mut self, call: lir::Instruction) {
        if let lir::Instruction::Call { func, args, dst } = call {
            self.label = func;

            // Save all caller-save registers on the stack
            // This needs cleanup after the register allocation
            save_regs!(
                [Rdi, Rsi, Rdx, Rcx, R8, R9, R10, R11, Rax],
                FnInstruction,
                self.arg_save,
                self.arg_recover
            );

            let mut push_setup = vec![];
            for (i, arg) in args.into_iter().enumerate() {
                if i < 6 {
                    // Fill the function argument registers
                    self.setup.push(FnInstruction::Movq {
                        src: Operand::LirOperand(arg),
                        dst: Operand::Reg(Amd64Reg::arg(i)),
                    });
                } else {
                    // Push the other args on the stack
                    push_setup.push(FnInstruction::Pushq {
                        src: Operand::LirOperand(arg),
                    });
                }
            }
            if !push_setup.is_empty() {
                // Remove the pushed args from stack after the call
                self.recover = Some(FnInstruction::Addq {
                    src: Tarval::mj_int((push_setup.len() * 8) as i64),
                    dst: Operand::Reg(Amd64Reg::Rsp),
                });

                // Rev the pushed args order: .., 8, 7, 6
                self.setup
                    .append(&mut push_setup.into_iter().rev().collect());
            }

            self.move_res = dst.map(|dst| FnInstruction::Movq {
                src: Operand::Reg(Amd64Reg::Rax),
                dst: Operand::LirOperand(lir::Operand::Slot(dst)),
            });
        } else {
            unreachable!("A FunctionCall can only be setup for a Call instruction")
        }
    }

    fn setup_stack_cconv(&mut self, call: lir::Instruction) {
        if let lir::Instruction::Call { func, args, dst } = call {
            self.label = func;

            // Save all caller-save registers on the stack
            // This needs cleanup after the register allocation
            save_regs!(
                [Rdi, Rsi, Rdx, Rcx, R8, R9, R10, R11, Rax],
                FnInstruction,
                self.arg_save,
                self.arg_recover
            );

            for arg in args.into_iter().rev() {
                self.setup.push(FnInstruction::Pushq {
                    src: Operand::LirOperand(arg),
                });
            }

            // Remove the pushed args from stack after the call
            self.recover = Some(FnInstruction::Addq {
                src: Tarval::mj_int((self.setup.len() * 8) as i64),
                dst: Operand::Reg(Amd64Reg::Rsp),
            });

            self.move_res = dst.map(|dst| FnInstruction::Movq {
                src: Operand::Reg(Amd64Reg::Rax),
                dst: Operand::LirOperand(lir::Operand::Slot(dst)),
            });
        } else {
            unreachable!("A FunctionCall can only be setup for a Call instruction")
        }
    }
}

pub struct Function {
    /// Number of arguments
    pub(super) nargs: usize,
    /// Calling convention
    pub(super) cconv: CallingConv,
    /// Setup of the function. Get's filled initially
    prolog: Vec<Instruction>,
    /// Save callee-save registers. A call to `self.save_callee_save_regs` is
    /// needed after the register allocation
    save_regs: Vec<Instruction>,
    /// Allocates stack memory. An extra function needs to be called
    allocate: Option<Instruction>,
    /// Restore callee-save registers. This will be setup together with
    /// `save_regs`
    restore_regs: Vec<Instruction>,
    /// Restore of previous stack pointer and return. Get's filled initially
    epilog: Vec<Instruction>,
}

impl Function {
    pub fn new(nargs: usize, cconv: CallingConv) -> Self {
        let mut function = Self {
            nargs,
            cconv,
            prolog: vec![],
            save_regs: vec![],
            allocate: None,
            restore_regs: vec![],
            epilog: vec![],
        };

        function.prolog.push(Instruction::Pushq {
            src: Operand::Reg(Amd64Reg::Rbp),
        });
        function.prolog.push(Instruction::Movq {
            src: MoveOperand::Operand(Operand::Reg(Amd64Reg::Rsp)),
            dst: MoveOperand::Operand(Operand::Reg(Amd64Reg::Rbp)),
        });

        function.epilog.push(Instruction::Movq {
            src: MoveOperand::Operand(Operand::Reg(Amd64Reg::Rbp)),
            dst: MoveOperand::Operand(Operand::Reg(Amd64Reg::Rsp)),
        });
        function.epilog.push(Instruction::Popq {
            dst: Operand::Reg(Amd64Reg::Rbp),
        });
        function.epilog.push(Instruction::Ret);

        function
    }

    #[allow(unused)]
    pub(super) fn allocate_stack(&mut self, slots: usize) {
        self.allocate = Some(Instruction::Subq {
            src: Operand::LirOperand(lir::Operand::Imm(Tarval::mj_int(8 * slots as i64))),
            dst: Operand::Reg(Amd64Reg::Rsp),
        });
    }

    /// This function should be called by the register allocator, after
    /// determining how many registers will be required for a function. If
    /// callee_save registers are needed to satisfy the register pressure,
    /// it will push these registers on the stack, before the function code
    /// is executed and restores them, after the function finished.
    ///
    /// The `num_regs_required` is the amount of registers that are required by
    /// this function, inclusive the reserved argument registers
    ///
    /// # Panics
    ///
    /// This function panics, if the number of required registers is higher,
    /// than the total available registers.
    pub fn save_callee_save_regs(&mut self, num_regs_required: usize) {
        // There are 5 callee save registers: %rbx, %r12-r15
        // %rbp is also callee save, but we never allocate this register
        // There are 10 caller save registers, but %rsp is reserved, so we need to save
        // registers if more than 9 registers are required.
        match num_regs_required {
            x if x < 10 => (), // Enough caller save registers available
            10 => save_regs!([Rbx], Instruction, self.save_regs, self.restore_regs),
            11 => save_regs!([Rbx, R12], Instruction, self.save_regs, self.restore_regs),
            12 => save_regs!(
                [Rbx, R12, R13],
                Instruction,
                self.save_regs,
                self.restore_regs
            ),
            13 => save_regs!(
                [Rbx, R12, R13, R14],
                Instruction,
                self.save_regs,
                self.restore_regs
            ),
            14 => save_regs!(
                [Rbx, R12, R13, R14, R15],
                Instruction,
                self.save_regs,
                self.restore_regs
            ),
            _ => unreachable!("More registers required than available"),
        }
    }

    pub fn allocate_registers(&self, graph: Ptr<lir::BlockGraph>) {
        let mut lva = LiveVariableAnalysis::new(self.cconv, graph);

        lva.run(graph.end_block);
        // log::debug!(
        //     "Liveness: {:#?}",
        //     lva.liveness
        //         .iter()
        //         .map(|(id, blocks)| (
        //             id,
        //             blocks
        //                 .iter()
        //                 .map(|block| format!("{}, {}", block.num, block._firm_num))
        //                 .collect::<Vec<_>>()
        //         ))
        //         .collect::<Vec<_>>()
        // );

        let mut lsa = self.build_lsa(lva);

        lsa.run();

        log::debug!("{:?}", lsa.var_location)
    }

    fn build_lsa(&self, lva: LiveVariableAnalysis) -> linear_scan::LinearScanAllocator {
        let mut instr_counter = 0;
        let mut map: HashMap<VarId, Vec<(usize, usize)>> = HashMap::new();
        let mut block_last_instr = vec![];
        for block in lva.postorder_blocks {
            for instr in block.instrs {
                for op in instr.src_operands() {
                    match op {
                        lir::Operand::Imm(_) => (),
                        _ => map
                            .entry(var_id(op))
                            .or_default()
                            .push((block.num, instr_counter)),
                    }
                }
                if let Some(dst) = instr.dst_operand() {
                    match dst {
                        lir::Operand::Imm(_) => unreachable!(),
                        _ => map
                            .entry(var_id(dst))
                            .or_default()
                            .push((block.num, instr_counter)),
                    }
                }
                instr_counter += 1;
            }
            block_last_instr.push(instr_counter - 1);
        }

        let mut var_live = BTreeSet::new();
        for (var_id, instrs) in map {
            let last_instr = instrs.iter().last().unwrap();
            let last_block_alive = lva.liveness.get(&var_id).map_or(last_instr.0, |blocks| {
                blocks.iter().max_by(|a, b| a.num.cmp(&b.num)).unwrap().num
            });
            let interval = Interval::new(
                instrs[0].1,
                if last_block_alive == last_instr.0 {
                    last_instr.1
                } else {
                    block_last_instr[last_block_alive]
                },
            );

            var_live.insert(linear_scan::LiveRange { var_id, interval });
        }

        log::debug!("{:?}", var_live);

        linear_scan::LinearScanAllocator::new(
            RegisterAllocator::new(self.nargs, self.cconv),
            var_live,
        )
    }
}

// fn gen_instrs(block: &lir::BasicBlock) -> linear_scan::Block {
//     let code = &block.code;
//     let mut reg_block = linear_scan::Block::default();
//     for lir::CopyPropagation { src, dst } in &code.copy_in {
//         reg_block.instrs.push(Instruction::Movq {
//             src:
// MoveOperand::Operand(Operand::LirOperand(lir::Operand::Slot(*src))),
//             dst: MoveOperand::Operand(Operand::LirOperand(lir::Operand::Slot(
//                 dst.allocated_in.regs[dst.num],
//             ))),
//         })
//     }
//     for instr in &code.body {
//         reg_block.instrs.append(&mut gen_instr(instr));
//     }
//     for lir::CopyPropagation { src, dst } in &code.copy_out {
//         reg_block.instrs.push(Instruction::Movq {
//             src:
// MoveOperand::Operand(Operand::LirOperand(lir::Operand::Slot(*src))),
//             dst: MoveOperand::Operand(Operand::LirOperand(lir::Operand::Slot(
//                 dst.allocated_in.regs[dst.num],
//             ))),
//         })
//     }
//     for leave in &code.leave {
//         reg_block.instrs.append(&mut gen_leave(&leave));
//     }
//
//     reg_block
// }
//
// // FIXME: this will also be done after the register allocation
// fn gen_leave(leave: &lir::Leave) -> Vec<Instruction> {
//     use super::lir::Leave::*;
//     match leave {
//         CondJmp {
//             op,
//             lhs,
//             rhs,
//             true_target,
//             false_target,
//         } => vec![
//             // FIXME: (Imm, Imm) case (other errorprone combinations?)
//             Instruction::Cmpq {
//                 lhs: Operand::LirOperand(*lhs),
//                 rhs: Operand::LirOperand(*rhs),
//             },
//             Instruction::Jmp {
//                 target: lir::gen_label(true_target),
//                 cond: lir::JmpKind::Conditional(*op),
//             },
//             Instruction::Jmp {
//                 target: lir::gen_label(false_target),
//                 cond: lir::JmpKind::Unconditional,
//             },
//         ],
//         Jmp { target } => vec![Instruction::Jmp {
//             target: lir::gen_label(target),
//             cond: lir::JmpKind::Unconditional,
//         }],
//         Return { value, end_block } => {
//             let mut ret = vec![];
//             if let Some(value) = value {
//                 ret.push(Instruction::Movq {
//                     src: MoveOperand::Operand(Operand::LirOperand(*value)),
//                     dst: MoveOperand::Operand(Operand::Reg(Amd64Reg::Rax)),
//                 });
//             }
//             ret.push(Instruction::Jmp {
//                 target: lir::gen_label(end_block),
//                 cond: lir::JmpKind::Unconditional,
//             });
//             ret
//         }
//     }
// }

// This function generates a `amd64::Instruction` from a `lir::Instruction`.
// This function needs to take care of following points:
//
// - a `lir::Instruction::Binop/Div/Mod` is a 3-address instruction.
//   - For all instructions `op src1, src2 -> dst(Slot)`: `dst != src1, src2`,
//     which means that a
//   move to `dst` is sometimes required:
//     - (slot1, slot2)   -> move slot2
//     - (slot, param)    -> move slot (commutative?)
//     - (slot, imm)      -> no move
//     - (param, slot)    -> move slot
//     - (param1, param2) -> see below
//     - (param, imm)     -> move imm
//     - (imm, slot)      -> no move
//     - (imm, param)     -> move imm (commutative?)
//     - (imm1, imm2)     -> move imm2
// - (param1, param2): Params should never be moved before a operation, because
//   the register allocator should handle parameters. The instruction selection
//   can't know which parameter should be moved from the stack, if a parameter
//   is already/not anymore in a register, ... Maybe a special case in the
//   `amd64::Instruction` enum operands is required for this.
// - Call instruction need to be destructured. This can be easily done by the
//   FuncntionCall struct, it just needs to be "flattened" afterwards.
//
// A remodelling of the `amd64::Instruction`+`Operand` enums is probably
// required.
// fn gen_instr(instr: &lir::Instruction) -> Vec<Instruction> {
//     vec![]
// }
