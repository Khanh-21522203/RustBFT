# RustBFT Diagrams

Auto-generated from codebase exploration. All diagrams use Mermaid (renders in GitHub/GitLab).

## Index

| File | Diagram Type | What it shows |
|------|-------------|---------------|
| [c4-context.md](c4-context.md) | C4 Level 1 Context | External actors and system boundary |
| [c4-container.md](c4-container.md) | C4 Level 2 Container | Internal components and their threads |
| [data-flow.md](data-flow.md) | Flowchart | Channel topology and shared state access |
| [consensus-sequence.md](consensus-sequence.md) | Sequence | BFT happy path + timeout/round-skip paths |
| [consensus-state-machine.md](consensus-state-machine.md) | State | Consensus step transitions and locking rules |
| [core-types.md](core-types.md) | Class | Domain types (Block, Vote, Proposal, ValidatorSet) |

## Quick Architecture Summary

```
External ──► RPC (Axum HTTP)
             P2P (TCP, ChaCha20)  ──► Command Router ──► Consensus Core
             TimerService          ◄──                  (single OS thread)
                                        │
                                    State Executor
                                    Contract Runtime (WASM)
                                    BlockStore (RocksDB)
                                    WAL (crash recovery)
```

The **Consensus Core** is a pure synchronous state machine with no I/O.
All async I/O is handled by Tokio and bridged via the **Command Router**.
