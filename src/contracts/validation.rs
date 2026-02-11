use anyhow::Result;
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct ValidationLimits {
    pub max_code_bytes: usize,
    pub max_memory_pages: u32, // 64KB/page, 256 pages = 16MB
}

impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            max_code_bytes: 512 * 1024,
            max_memory_pages: 256,
        }
    }
}

#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("code too large")]
    CodeTooLarge,
    #[error("invalid wasm module")]
    InvalidWasm,
    #[error("float instructions are not allowed")]
    FloatNotAllowed,
    #[error("simd is not allowed")]
    SimdNotAllowed,
    #[error("threads are not allowed")]
    ThreadsNotAllowed,
    #[error("bulk memory is not allowed (MVP restriction)")]
    BulkMemoryNotAllowed,
    #[error("unknown import: module={module} name={name}")]
    UnknownImport { module: String, name: String },
    #[error("start function is not allowed")]
    StartNotAllowed,
    #[error("memory exceeds limit")]
    MemoryTooLarge,
}

pub fn validate_wasm_module(bytes: &[u8], limits: &ValidationLimits) -> Result<(), ValidationError> {
    if bytes.len() > limits.max_code_bytes {
        return Err(ValidationError::CodeTooLarge);
    }

    let mut has_start = false;
    let mut max_pages: Option<u32> = None;

    let parser = wasmparser::Parser::new(0);
    for payload in parser.parse_all(bytes) {
        let payload = payload.map_err(|_| ValidationError::InvalidWasm)?;
        match payload {
            wasmparser::Payload::StartSection { .. } => {
                has_start = true;
            }
            wasmparser::Payload::ImportSection(s) => {
                for imp in s {
                    let imp = imp.map_err(|_| ValidationError::InvalidWasm)?;
                    // Only allow imports from namespace "rustbft"
                    if imp.module != "rustbft" {
                        return Err(ValidationError::UnknownImport {
                            module: imp.module.to_string(),
                            name: imp.name.to_string(),
                        });
                    }
                }
            }
            wasmparser::Payload::MemorySection(s) => {
                for mem in s {
                    let mem = mem.map_err(|_| ValidationError::InvalidWasm)?;
                    // wasmparser v0.120: initial/maximum are direct fields (u64)
                    let declared_max = mem.maximum.unwrap_or(mem.initial) as u32;
                    max_pages = Some(max_pages.map(|m: u32| m.max(declared_max)).unwrap_or(declared_max));
                }
            }
            wasmparser::Payload::CodeSectionEntry(body) => {
                let mut op_reader = body.get_operators_reader().map_err(|_| ValidationError::InvalidWasm)?;
                while !op_reader.eof() {
                    let op = op_reader.read().map_err(|_| ValidationError::InvalidWasm)?;
                    use wasmparser::Operator::*;
                    match op {
                        // Reject any floating point instructions
                        F32Abs | F32Add | F32Ceil | F32ConvertI32S | F32ConvertI32U | F32ConvertI64S | F32ConvertI64U
                        | F32Copysign | F32DemoteF64 | F32Div | F32Eq | F32Floor | F32Ge | F32Gt | F32Le | F32Lt
                        | F32Max | F32Min | F32Mul | F32Ne | F32Nearest | F32Neg | F32ReinterpretI32 | F32Sqrt
                        | F32Sub | F32Trunc | F64Abs | F64Add | F64Ceil | F64ConvertI32S | F64ConvertI32U | F64ConvertI64S
                        | F64ConvertI64U | F64Copysign | F64Div | F64Eq | F64Floor | F64Ge | F64Gt | F64Le | F64Lt
                        | F64Max | F64Min | F64Mul | F64Ne | F64Nearest | F64Neg | F64PromoteF32 | F64ReinterpretI64
                        | F64Sqrt | F64Sub | F64Trunc => {
                            return Err(ValidationError::FloatNotAllowed);
                        }

                        // SIMD (wasmparser uses V128 ops)
                        V128Load { .. } | V128Store { .. } | I8x16Shuffle { .. }
                        | I8x16Swizzle | I8x16Splat | I16x8Splat | I32x4Splat | I64x2Splat
                        | F32x4Splat | F64x2Splat => {
                            return Err(ValidationError::SimdNotAllowed);
                        }

                        // Bulk memory ops (MVP restriction)
                        MemoryInit { .. } | DataDrop { .. } | MemoryCopy { .. } | MemoryFill { .. } | TableInit { .. } | ElemDrop { .. }
                        | TableCopy { .. } => {
                            return Err(ValidationError::BulkMemoryNotAllowed);
                        }

                        // Threads (rough detection: atomic ops)
                        I32AtomicLoad { .. } | I64AtomicLoad { .. } | I32AtomicStore { .. } | I64AtomicStore { .. }
                        | I32AtomicRmwAdd { .. } | I64AtomicRmwAdd { .. } | I32AtomicRmwSub { .. } | I64AtomicRmwSub { .. }
                        | I32AtomicRmwAnd { .. } | I64AtomicRmwAnd { .. } | I32AtomicRmwOr { .. } | I64AtomicRmwOr { .. }
                        | I32AtomicRmwXor { .. } | I64AtomicRmwXor { .. } | I32AtomicRmwXchg { .. } | I64AtomicRmwXchg { .. }
                        | I32AtomicRmwCmpxchg { .. } | I64AtomicRmwCmpxchg { .. } | AtomicFence { .. } => {
                            return Err(ValidationError::ThreadsNotAllowed);
                        }

                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    if has_start {
        return Err(ValidationError::StartNotAllowed);
    }

    if let Some(pages) = max_pages {
        if pages > limits.max_memory_pages {
            return Err(ValidationError::MemoryTooLarge);
        }
    }

    Ok(())
}
