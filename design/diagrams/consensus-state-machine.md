# Consensus State Machine

State transitions for a single validator in the BFT protocol.
Source: `src/consensus/state.rs` — `Step` enum and `enter_*()` methods.

```mermaid
stateDiagram-v2
    [*] --> NewHeight : node starts / block committed

    NewHeight --> Propose : enter_round(0)

    Propose --> Prevote : ProposalReceived (valid block + sig)
    Propose --> Prevote : TimeoutPropose (vote nil)

    Prevote --> Precommit : >2/3 Prevotes for block (polka formed)\nlock block, lock round
    Prevote --> Precommit : >2/3 Prevotes for nil
    Prevote --> Precommit : TimeoutPrevote (vote nil)

    Precommit --> Commit : >2/3 Precommits for block\ncheck_commit_quorum()
    Precommit --> Propose : TimeoutPrecommit → enter_round(R+1)

    Commit --> NewHeight : BlockExecuted + PersistBlock\napply validator updates

    Prevote --> Propose : >1/3 votes at higher round R′\nenter_round(R′)
    Precommit --> Propose : >1/3 votes at higher round R′\nenter_round(R′)
```

## Step Descriptions

| Step | Entry Condition | Actions |
|------|----------------|---------|
| **NewHeight** | Block committed at H | Reset state, apply validator_updates, schedule new height |
| **Propose** | Entering round R | If proposer: build/re-propose block. Schedule TimeoutPropose |
| **Prevote** | Proposal received or TimeoutPropose | Vote `block_hash` (if valid) or `nil`. Schedule TimeoutPrevote |
| **Precommit** | Polka formed or TimeoutPrevote | Vote `block_hash` (if locked) or `nil`. Schedule TimeoutPrecommit |
| **Commit** | >2/3 Precommits for block | Execute block → persist → advance height |

## Locking Rules (Safety Invariants)

```mermaid
flowchart TD
    A["Enter Precommit"] --> B{"Polka for block\nat this round?"}
    B -->|Yes| C["Lock block\nlocked_round = R\nlocked_block = B"]
    B -->|No| D{"locked_block\nexists?"}
    D -->|Yes| E["Precommit\nlocked_block"]
    D -->|No| F["Precommit nil"]
    C --> G["Precommit block"]

    H["New Proposal received\nwith valid_round R′"] --> I{"valid_round R′\n≥ locked_round?"}
    I -->|"Yes (polka at R′)"| J["Unlock\naccept new proposal"]
    I -->|No| K["Reject proposal\nkeep locked block"]
```

## Timeout Backoff

Each step's timeout grows linearly with the round number:

```
timeout(step, round) = base_timeout_<step>_ms + round × timeout_delta_ms
```

| Step | Default base | Default delta |
|------|-------------|--------------|
| Propose | configurable | configurable |
| Prevote | configurable | configurable |
| Precommit | configurable | configurable |

Source: `ConsensusConfig` in `src/consensus/mod.rs`
