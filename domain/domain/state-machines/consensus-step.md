# Consensus Step State Machine

Source: `src/consensus/state.rs` — `enum Step` + `ConsensusCore`

## State Diagram

```mermaid
stateDiagram-v2
    [*] --> NewHeight : startup or BlockExecuted

    NewHeight --> Propose : enter_new_height() calls enter_propose()

    Propose --> Prevote : proposal received while in Propose
    Propose --> Prevote : txs received, proposal built and broadcast (proposer path)
    Propose --> Prevote : timeout_propose fires, no proposal, prevote nil

    Prevote --> Precommit : polka for block (over 2/3 prevotes for same hash)
    Prevote --> Precommit : polka nil (over 2/3 prevotes for nil)
    Prevote --> Precommit : timeout_prevote fires after polka-any threshold

    Precommit --> Commit : quorum precommits for block
    Precommit --> Propose : timeout_precommit fires, start next round

    Commit --> NewHeight : BlockExecuted received from state executor
```

## Forbidden Transitions

| From | To | Reason |
|---|---|---|
| `Commit` | `Propose` (same height) | Must pass through `NewHeight` to reset locked/valid state |
| `Prevote` | `Propose` (same round) | BFT: once prevoting, cannot re-enter propose for that round |
| `Precommit` | `Prevote` (same round) | One-way progression within a round |
| `NewHeight` | `Commit` | Must pass through Propose, Prevote, Precommit |
| any step | `NewHeight` (mid-round) | Only `handle_block_executed` may call `enter_new_height()` |

## Round Skip

If over 1/3 of voting power is seen at a **higher** round via incoming votes, `check_round_skip()` fires `enter_round(target_round)` from any step except `Commit`. This resets per-round flags but does **not** reset `locked_block` or `valid_block`.

```mermaid
stateDiagram-v2
    Propose --> Propose : round skip, over 1/3 votes at higher round
    Prevote --> Propose : round skip, over 1/3 votes at higher round
    Precommit --> Propose : round skip, over 1/3 votes at higher round
```

## Notes

- `locked_block` and `valid_block` survive across rounds; only reset on new height.
- `prevoted_this_round` and `precommitted_this_round` prevent double-voting (invariants I1/I2).
- WAL is written before broadcasting a vote or proposal for crash recovery.
- Timeout duration grows linearly: `base_ms + round * delta_ms`.

> **Verified against:** `src/consensus/state.rs` — `enter_propose()`, `enter_prevote()`, `enter_precommit()`, `commit_block_hash()`, `enter_new_height()`, `handle_block_executed()`, `handle_timeout_propose/prevote/precommit()`.
