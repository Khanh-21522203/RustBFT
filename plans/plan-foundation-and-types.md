# Feature: Foundation & Types

## 1. Purpose

Foundation & Types is the base layer that every other module in RustBFT depends on. It defines all shared data structures, provides the deterministic canonical serialization format, implements cryptographic primitives (Ed25519, SHA-256), and parses the genesis configuration.

Without this layer, each subsystem would define its own `Block`, `Vote`, or `Hash`, causing incompatible types and subtle encoding bugs that break consensus. By defining everything in one place, the Rust compiler enforces type safety across the entire codebase, and a single canonical encoding guarantees that `sha256(serialize(X))` produces the same bytes on every node regardless of platform.

This is the prerequisite phase for all other features. Nothing compiles until this layer is complete.

## 2. Responsibilities

- Define all shared types: `Block`, `BlockHeader`, `Transaction`, `Vote`, `Proposal`, `ValidatorSet`, `Validator`, `Address`, `Hash`, `Signature`, `Receipt`, `CommitInfo`, `Event`
- Implement canonical, deterministic binary serialization for every type (custom format, not serde-derived)
- Implement Ed25519 keypair generation, signing, and signature verification
- Implement SHA-256 hashing and binary Merkle tree root computation
- Derive `ValidatorId` (32 bytes) and `Address` (20 bytes) from Ed25519 public keys
- Parse and validate `genesis.json`: initial validators, initial accounts, admin addresses, chain params
- Set up the single-crate project with module stubs for all layers
- Configure CI pipeline: `cargo fmt`, `cargo clippy`, `cargo test`, `cargo deny`
- Ban `HashMap` and `HashSet` crate-wide via `#![deny(clippy::disallowed_types)]`

## 3. Non-Responsibilities

- No network I/O
- No disk access
- No consensus logic
- No business rules beyond genesis validation
- No `serde` derives on consensus-path types (only on config/genesis types)

## 4. Architecture Design

```
+----------------------------------------------------------+
|            All RustBFT Modules                            |
|  consensus | p2p | state | contracts | storage | rpc     |
+---------------------------+------------------------------+
                            |
                   use src::types::*
                   use src::crypto::*
                            |
              +-------------+-------------+
              |    src/types/             |
              |  Block    Transaction     |
              |  Vote     Proposal        |
              |  ValidatorSet  Receipt    |
              |  CanonicalEncode trait    |
              +-------------+-------------+
                            |
              +-------------+-------------+
              |    src/crypto/            |
              |  sha256()  merkle_root()  |
              |  Ed25519 sign / verify    |
              |  address_from_pubkey()    |
              +----------------------------+
```

`src/types/` depends only on `src/crypto/`. Both depend only on the Rust standard library and a small set of pinned crates (`ed25519-dalek`, `sha2`).

## 5. Core Data Structures (Rust)

```rust
// src/types/mod.rs
pub type Hash = [u8; 32];
pub type Address = [u8; 20];
pub type ValidatorId = [u8; 32];
pub type Bytes = Vec<u8>;
pub type Signature = [u8; 64];

// src/types/block.rs
pub struct Block {
    pub header: BlockHeader,
    pub txs: Vec<SignedTransaction>,
    pub last_commit: CommitInfo,
}

pub struct BlockHeader {
    pub height: u64,
    pub timestamp: u64,           // Unix millis, advisory only
    pub prev_block_hash: Hash,
    pub proposer: ValidatorId,
    pub validator_set_hash: Hash,
    pub state_root: Hash,
    pub tx_merkle_root: Hash,
}

pub struct CommitInfo {
    pub height: u64,
    pub signatures: Vec<(ValidatorId, Signature)>,
}

// src/types/transaction.rs
pub struct SignedTransaction {
    pub tx: Transaction,
    pub signature: Signature,
}

pub enum Transaction {
    Transfer(Transfer),
    ContractDeploy(ContractDeploy),
    ContractCall(ContractCall),
    ValidatorUpdate(ValidatorUpdate),
}

pub struct Transfer {
    pub from: Address,
    pub to: Address,
    pub amount: u128,
    pub nonce: u64,
    pub gas_limit: u64,
}

pub struct ContractDeploy {
    pub from: Address,
    pub code: Bytes,
    pub init_args: Bytes,
    pub nonce: u64,
    pub gas_limit: u64,
}

pub struct ContractCall {
    pub from: Address,
    pub to: Address,
    pub input: Bytes,
    pub amount: u128,
    pub nonce: u64,
    pub gas_limit: u64,
}

pub struct ValidatorUpdate {
    pub from: Address,
    pub action: ValidatorAction,
    pub validator_id: ValidatorId,
    pub public_key: Option<Ed25519PublicKey>,
    pub new_power: Option<u64>,
    pub nonce: u64,
    pub gas_limit: u64,
}

pub enum ValidatorAction { Add, Remove, UpdatePower }

pub struct Receipt {
    pub success: bool,
    pub gas_used: u64,
    pub logs: Vec<Event>,
    pub error: Option<String>,
}

pub struct Event {
    pub topic: String,
    pub data: Bytes,
}

// src/types/vote.rs
pub struct SignedVote {
    pub vote: Vote,
    pub signature: Signature,
}

pub struct Vote {
    pub vote_type: VoteType,
    pub height: u64,
    pub round: u32,
    pub block_hash: Option<Hash>,  // None = nil vote
    pub validator: ValidatorId,
}

pub enum VoteType { Prevote, Precommit }

// src/types/proposal.rs
pub struct SignedProposal {
    pub proposal: Proposal,
    pub signature: Signature,
}

pub struct Proposal {
    pub height: u64,
    pub round: u32,
    pub block: Block,
    pub valid_round: i32,   // -1 if no prior valid round
    pub proposer: ValidatorId,
}

// src/types/validator.rs
pub struct Validator {
    pub id: ValidatorId,
    pub public_key: Ed25519PublicKey,
    pub voting_power: u64,
    pub address: Address,
}

pub struct ValidatorSet {
    pub validators: BTreeMap<ValidatorId, Validator>,  // BTreeMap, never HashMap
    pub total_power: u64,
}

// src/types/genesis.rs
pub struct GenesisConfig {
    pub chain_id: String,
    pub genesis_time: u64,
    pub initial_validators: Vec<GenesisValidator>,
    pub initial_accounts: Vec<GenesisAccount>,
    pub admin_addresses: Vec<Address>,
    pub chain_params: ChainParams,
}

pub struct ChainParams {
    pub max_block_gas: u64,
    pub max_block_bytes: u64,
    pub max_tx_bytes: u64,
}
```

## 6. Public Interfaces

```rust
// Canonical serialization trait (src/types/serialization.rs)
pub trait CanonicalEncode {
    fn canonical_encode(&self) -> Vec<u8>;
}

pub trait CanonicalDecode: Sized {
    fn canonical_decode(buf: &[u8]) -> Result<Self, DecodeError>;
}

// Hashing shorthand on any encodable type
pub trait Hashable: CanonicalEncode {
    fn hash(&self) -> Hash {
        sha256(&self.canonical_encode())
    }
}

// Crypto (src/crypto/)
pub fn sha256(data: &[u8]) -> Hash;
pub fn sha256_concat(a: &[u8], b: &[u8]) -> Hash;
pub fn merkle_root(leaves: &[Hash]) -> Hash;  // returns zero hash for empty slice

pub fn generate_keypair() -> (Ed25519PrivateKey, Ed25519PublicKey);
pub fn sign(key: &Ed25519PrivateKey, message: &[u8]) -> Signature;
pub fn verify(pubkey: &Ed25519PublicKey, message: &[u8], sig: &Signature) -> bool;
pub fn validator_id_from_pubkey(pubkey: &Ed25519PublicKey) -> ValidatorId;
pub fn address_from_pubkey(pubkey: &Ed25519PublicKey) -> Address;

// Genesis (src/types/genesis.rs)
pub fn load_genesis(path: &Path) -> Result<GenesisConfig, GenesisError>;
pub fn validate_genesis(cfg: &GenesisConfig) -> Result<(), GenesisError>;
pub fn genesis_state_root(cfg: &GenesisConfig) -> Hash;
```

## 7. Internal Algorithms

### Canonical Encoding Rules
The encoding is custom binary, not serde. Rules applied in order:
```
u8          → 1 byte big-endian
u32         → 4 bytes big-endian
u64         → 8 bytes big-endian
u128        → 16 bytes big-endian
i32         → 4 bytes big-endian two's complement
[u8; N]     → N raw bytes (fixed-size arrays)
Vec<u8>     → u32 BE length + raw bytes
String      → u32 BE length + UTF-8 bytes
Option<T>   → 0x00 (None) | 0x01 + encode(T) (Some)
Vec<T>      → u32 BE count + encode(T) for each element
enum        → u8 discriminant + fields of variant
struct      → fields in declaration order, no padding
```

### Merkle Root Computation
```
merkle_root([]):           return [0u8; 32]
merkle_root([h]):          return h
merkle_root([h1, h2]):     return sha256(h1 || h2)
merkle_root(leaves):       pad to next power of 2 with zero hashes,
                           then binary tree reduce with sha256_concat
```

### ValidatorId and Address Derivation
```
public_key_bytes = ed25519_pubkey.to_bytes()  // 32 bytes
validator_id     = sha256(public_key_bytes)    // 32 bytes
address          = sha256(public_key_bytes)[0..20]  // first 20 bytes
```

### Genesis Validation
1. `chain_id` must be non-empty
2. At least one validator with `voting_power > 0`
3. No duplicate `ValidatorId` in `initial_validators`
4. All Ed25519 public keys are valid (decoded without error)
5. `max_block_gas > 0`, `max_block_bytes > 0`, `max_tx_bytes > 0`
6. `genesis_state_root` = `merkle_root(sha256(canonical_encode(account)) for each account sorted by address)`

## 8. Persistence Model

No persistence in this layer. Types are value types; serialization is stateless. The genesis file is read once at startup by the node binary.

## 9. Concurrency Model

All types are plain Rust structs with no interior mutability. `Hash`, `Address`, `ValidatorId`, and `Signature` are `Copy`. All types implement `Send + Sync` automatically. No locks, channels, or threads.

## 10. Configuration

No runtime configuration. Constants:

```rust
pub const HASH_SIZE: usize = 32;
pub const ADDRESS_SIZE: usize = 20;
pub const SIGNATURE_SIZE: usize = 64;
pub const PUBKEY_SIZE: usize = 32;
```

CI configuration (`.github/workflows/ci.yml`):
```yaml
stages: [fmt-check, clippy, test, cargo-deny]
rust: stable
deny: licenses + advisories
```

## 11. Observability

No metrics or logging in this layer. Errors are typed (`DecodeError`, `GenesisError`) with descriptive messages for callers to log. All error types implement `std::error::Error`.

## 12. Testing Strategy

- **`TestSerializationRoundTrip`**: for every type, encode then decode, assert structural equality
- **`TestCanonicalEncoding`**: two identical values produce identical bytes; known byte sequences match expected
- **`TestSha256KnownVectors`**: RFC test vectors for SHA-256 (empty string, "abc", etc.)
- **`TestMerkleRoot`**: empty slice → zero hash; single leaf → leaf hash; two leaves → known output; 4 leaves → known root
- **`TestEd25519KnownVectors`**: sign/verify with RFC 8032 test vectors; wrong key returns false
- **`TestValidatorIdDerived`**: same pubkey → same ValidatorId every time
- **`TestAddressDerived`**: same pubkey → same Address; Address is first 20 bytes of ValidatorId
- **`TestGenesisParseValid`**: well-formed genesis.json parses without error
- **`TestGenesisValidation`**: duplicate validator → error; zero voting power → error; missing chain_id → error
- **`TestBTreeMapOrder`**: `ValidatorSet.validators` iterates in deterministic sorted order

All tests use only `cargo test`; no external test frameworks.

## 13. Open Questions

None.
