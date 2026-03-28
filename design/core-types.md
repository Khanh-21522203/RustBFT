# Core Types and Serialization

## Purpose

Defines the canonical data structures shared across all modules (blocks, transactions, votes, proposals, validators) and the binary serialization codec used for P2P wire encoding and storage.

## Scope

**In scope:**
- `Block`, `BlockHeader`, `CommitInfo` — the unit of consensus agreement
- `Transaction` enum — Transfer, ContractDeploy, ContractCall, ValidatorUpdate
- `Vote`, `SignedVote`, `Evidence` — consensus voting messages
- `Proposal`, `SignedProposal` — block proposal messages
- `Validator`, `ValidatorSet`, `ValidatorUpdate`, `ValidatorSetPolicy` — validator membership
- `Hash ([u8; 32])`, `Address ([u8; 20])` — primitive value types
- Binary `Encoder`/`Decoder` codec for block, proposal, and vote serialization

**Out of scope:**
- JSON serialization (handled per-module via `serde` derive)
- Transaction decoding for state execution (separate wire format in `src/state/tx.rs`)
- Cryptographic signing of votes/proposals (done by `crypto::ed25519`)

## Data Model

**`Hash`** (`src/types/hash.rs`): `Hash(pub [u8; 32])` — `Hash::ZERO = Hash([0u8; 32])`.

**`Address`** (`src/types/address.rs`): `Address(pub [u8; 20])` — `Address::ZERO`.

**`Block`** (`src/types/block.rs`):
- `header: BlockHeader`
- `txs: Vec<Vec<u8>>` — raw tx bytes; no schema enforced at this layer
- `last_commit: Option<CommitInfo>` — precommit signatures for previous block

**`BlockHeader`**:
- `height: u64`, `timestamp_ms: u64` (advisory only)
- `prev_block_hash: Hash`, `proposer: ValidatorId`, `validator_set_hash: Hash`
- `state_root: Hash`, `tx_merkle_root: Hash`
- Note: `tx_merkle_root` is never computed — always `Hash::ZERO` in practice

**`CommitInfo`** (`src/types/block.rs`):
- `height: u64`, `round: u32`, `block_hash: Hash`
- `signatures: Vec<SignedVote>` — the >2/3 precommit set

**`Vote`** (`src/types/vote.rs`):
- `vote_type: VoteType` — `Prevote` or `Precommit`
- `height: u64`, `round: u32`
- `block_hash: Option<Hash>` — `None` represents a nil vote
- `validator: ValidatorId`

**`SignedVote`**: `{ vote: Vote, signature: [u8; 64] }`

**`Evidence`**: `{ vote_a: SignedVote, vote_b: SignedVote }` — equivocation proof

**`Proposal`** (`src/types/proposal.rs`):
- `height: u64`, `round: u32`
- `block: Block` — the proposed block (inline)
- `valid_round: i32` — -1 if none (Tendermint `vr` field)
- `proposer: ValidatorId`

**`SignedProposal`**: `{ proposal: Proposal, signature: [u8; 64] }`

**`ValidatorId`** (`src/types/validator.rs`): `ValidatorId(pub [u8; 32])` — the ed25519 verify key bytes.

**`Validator`**: `{ id: ValidatorId, voting_power: u64 }`

**`ValidatorSet`**:
- `validators: BTreeMap<ValidatorId, Validator>` — sorted for determinism
- `total_power: u64` — cached sum
- `set_hash: Hash` — `sha256(sorted(id || power.to_be_bytes()))` for each validator
- Custom serde: serialized as `Vec<Validator>` (JSON map keys cannot be byte arrays)

**`ValidatorSetPolicy`** (safety constraints for `apply_updates`):
- `max_power_change_num/den: u64` — default 1/3 of total power per block
- `min_validators: usize` — default 1
- `max_validators: usize` — default 150

**`ValidatorUpdate`**: `{ id: ValidatorId, new_power: u64 }` — `new_power = 0` means remove

**`Transaction`** (`src/types/transaction.rs`) — high-level enum (used outside `state/`):
- `Transfer(TransferTx)`, `ContractDeploy(ContractDeployTx)`, `ContractCall(ContractCallTx)`, `ValidatorUpdate(ValidatorUpdateTx)`
- `gas_limit()`, `sender()`, `kind()` accessors
- Note: this enum is defined but not used in state execution — `src/state/tx.rs` has its own parallel wire format

**Binary Codec** (`src/types/serialization.rs`):
- `Encoder` — `put_u8/u32/u64/bytes32/bytes64/vec/opt_hash`, `into_bytes()`
- `Decoder` — `get_u8/u32/u64/bytes32/bytes64/vec/opt_hash`
- `encode_block(block) -> Vec<u8>` / `decode_block(data) -> Result<Block, CodecError>`
  - Block format: `[header fields][n_txs: u32][tx_i: u32-len-prefix]*`
  - `CommitInfo` (`last_commit`) is NOT encoded; always decoded as `None`
- `encode_signed_proposal(sp) -> Vec<u8>` / `decode_signed_proposal`
  - Format: `[height: u64][round: u32][block_bytes: vec][valid_round: u32 (0xFFFF_FFFF = -1)][proposer: 32B][sig: 64B]`
- `encode_signed_vote(sv) -> Vec<u8>` / `decode_signed_vote`
  - Format: `[vote_type: u8][height: u64][round: u32][block_hash: opt_hash][validator: 32B][sig: 64B]`

## Interfaces and Contracts

`ValidatorSet::apply_updates(updates, policy) -> Result<ValidatorSet, ValidatorSetError>`:
- Validates power delta ≤ `total_power × max_power_change_num / max_power_change_den`
- Enforces min/max validator count and non-zero total power
- Returns `Err` on any violation; does not mutate in-place
- `new_power = 0` removes validator; fails with `RemoveNonExistent` if not present

`ValidatorSet::compute_hash() -> Hash`:
- Iterates `BTreeMap` in key order → `sha256(concat(id.0 || power.to_be_bytes()))` for each validator

`tx_hash(_tx: &Transaction) -> Hash`:
- Always returns `Hash([0u8; 32])` — placeholder, not implemented

## Dependencies

**Internal modules used by types:**
- `src/crypto/hash.rs` — `sha256` used in `ValidatorSet::compute_hash`

**External crates:**
- `serde` with `derive` — all types implement `Serialize`/`Deserialize`
- `serde_bytes` — `#[serde(with = "serde_bytes")]` for `signature: [u8; 64]` fields
- `thiserror` — `ValidatorSetError`, `CodecError`

## Failure Modes and Edge Cases

- **`encode_block` drops `last_commit`:** Round-trip through `encode_block` → `decode_block` loses commit signatures. Stored blocks have no commit proof.
- **`tx_hash` always zero:** Cannot deduplicate transactions by hash.
- **`tx_merkle_root` always zero in `BlockHeader`:** No transaction Merkle proof possible.
- **`valid_round = -1` encoding:** Encoded as `0xFFFF_FFFF (u32::MAX)` in the block; decoded back to -1. Rounds above `2^31 - 2` are ambiguous.
- **`ValidatorId` from verify key:** The key bytes are the identity — key rotation is not supported without changing `ValidatorId`.
- **`ValidatorSet` serde via Vec:** Deserialization reconstructs the BTreeMap from a list; order is re-sorted by ValidatorId on load.
- **`Transaction` type in `types/` vs `state/tx.rs`:** Two parallel transaction representations exist. The `types/transaction.rs` enum is not used in execution and has no binary serialization. `state/tx.rs` has its own binary wire format used by `StateExecutor`.

## Observability and Debugging

No observability at the types layer. Debugging is done by inspecting values in the modules that use them.

## Risks and Notes

- Two transaction type definitions (`types/transaction.rs` and `state/tx.rs`) that are not connected — `SignatureBytes = [u8; 64]` in `types/transaction.rs` is never verified anywhere.
- `timestamp_ms` in `BlockHeader` is advisory and set to 0 in MVP — no wall-clock time in blocks.
- `prev_block_hash` in `BlockHeader` is always `Hash::ZERO` in MVP — no chain linking.

Changes:

