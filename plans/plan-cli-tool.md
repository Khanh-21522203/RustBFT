# Feature: CLI Tool (rustbft-cli)

## 1. Purpose

The `rustbft-cli` binary is the operator and developer command-line interface for interacting with a running RustBFT node. It constructs, signs, and submits transactions; queries blockchain state; and manages local key files. It communicates exclusively through the node's JSON-RPC 2.0 HTTP API and has no direct access to node internals.

## 2. Responsibilities

- Construct and canonically encode all transaction types: `Transfer`, `ContractDeploy`, `ContractCall`, `ValidatorUpdate`
- Sign transactions with a local Ed25519 private key loaded from a key file
- Submit signed transactions to a node via `broadcast_tx` or `broadcast_tx_commit`
- Query committed state: block by height/hash, transaction by hash, account balance and nonce, contract state, validator set, node status
- Deploy a WASM contract from a local `.wasm` file and report the resulting contract address
- Call a deployed contract and decode the output
- Manage key files: generate a new key pair, display the public key and node_id derived from a key file
- Output results as JSON or human-readable text (configurable via `--output json|text`)
- Return non-zero exit codes on any error, with a descriptive message on stderr

## 3. Non-Responsibilities

- Does not connect to the node via any mechanism other than HTTP JSON-RPC
- Does not validate transactions statefully (no nonce checking, no balance checking) — the node's mempool does that
- Does not run a node or participate in consensus
- Does not store wallet state or track nonces automatically (caller must supply the correct nonce)
- Does not support batch transaction submission for MVP
- Does not implement any ABI codec beyond the minimal selector + raw args encoding

## 4. Architecture Design

```
$ rustbft-cli tx transfer --from alice --to bob --amount 1000 ...

main()
  |
  ├── parse CLI (clap: subcommand tree)
  ├── load config (~/.rustbft/config.toml or --config flag)
  ├── load key file (--key-file flag)
  |
  ├── Commands:
  |     tx transfer      → build_transfer_tx → sign → broadcast
  |     tx deploy        → load wasm file → build_deploy_tx → sign → broadcast
  |     tx call          → build_call_tx → sign → broadcast
  |     tx validator     → build_validator_update_tx → sign → broadcast
  |     query account    → rpc_call("get_account") → print
  |     query block      → rpc_call("get_block") → print
  |     query tx         → rpc_call("get_tx") → print
  |     query contract   → rpc_call("query_contract") → print
  |     query validators → rpc_call("get_validators") → print
  |     status           → rpc_call("status") → print
  |     keys generate    → Ed25519::generate() → write file
  |     keys show        → load key → print public key + node_id
  |
  └── RpcClient: HTTP POST to --node flag (default: http://localhost:26657)
```

## 5. Core Data Structures (Rust)

```rust
// src/cli/mod.rs

#[derive(Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    #[arg(long, default_value = "http://localhost:26657")]
    pub node: String,

    #[arg(long, default_value = "text")]
    pub output: OutputFormat,
}

pub enum OutputFormat { Text, Json }

#[derive(Subcommand)]
pub enum Command {
    Tx(TxCommand),
    Query(QueryCommand),
    Status,
    Keys(KeysCommand),
}

#[derive(Subcommand)]
pub enum TxCommand {
    Transfer {
        #[arg(long)] from: String,   // address hex
        #[arg(long)] to: String,
        #[arg(long)] amount: u128,
        #[arg(long)] nonce: u64,
        #[arg(long)] key_file: PathBuf,
        #[arg(long, default_value = "21000")] gas_limit: u64,
        #[arg(long, default_value = "false")] wait: bool,  // broadcast_tx_commit
    },
    Deploy {
        #[arg(long)] from: String,
        #[arg(long)] code: PathBuf,           // path to .wasm file
        #[arg(long, default_value = "")] init_args: String,   // hex
        #[arg(long)] nonce: u64,
        #[arg(long)] key_file: PathBuf,
        #[arg(long, default_value = "1000000")] gas_limit: u64,
        #[arg(long, default_value = "false")] wait: bool,
    },
    Call {
        #[arg(long)] from: String,
        #[arg(long)] to: String,              // contract address
        #[arg(long)] input: String,           // hex-encoded ABI input
        #[arg(long, default_value = "0")] value: u128,
        #[arg(long)] nonce: u64,
        #[arg(long)] key_file: PathBuf,
        #[arg(long, default_value = "100000")] gas_limit: u64,
        #[arg(long, default_value = "false")] wait: bool,
    },
    Validator {
        #[arg(long)] from: String,
        #[arg(long)] action: ValidatorAction,  // add | remove | update-power
        #[arg(long)] validator_id: Option<String>,
        #[arg(long)] pubkey: Option<String>,
        #[arg(long)] voting_power: Option<u64>,
        #[arg(long)] nonce: u64,
        #[arg(long)] key_file: PathBuf,
        #[arg(long, default_value = "false")] wait: bool,
    },
}

#[derive(Subcommand)]
pub enum QueryCommand {
    Account  { address: String, #[arg(long)] height: Option<u64> },
    Block    { #[arg(long)] height: Option<u64>, #[arg(long)] hash: Option<String> },
    Tx       { hash: String },
    Contract { address: String, input: String, #[arg(long)] height: Option<u64> },
    Validators { #[arg(long)] height: Option<u64> },
}

#[derive(Subcommand)]
pub enum KeysCommand {
    Generate { #[arg(long)] output: PathBuf },
    Show     { #[arg(long)] key_file: PathBuf },
}

// RPC client
pub struct RpcClient {
    endpoint: String,
    http: reqwest::Client,
}
```

## 6. Public Interfaces

```rust
// src/cli/rpc.rs

impl RpcClient {
    pub fn new(endpoint: String) -> Self;

    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
    ) -> Result<R, CliError>;
}

// src/cli/tx.rs

pub fn build_transfer(from: Address, to: Address, amount: u128, nonce: u64, gas_limit: u64)
    -> Transaction;

pub fn build_deploy(from: Address, code: Vec<u8>, init_args: Vec<u8>, nonce: u64, gas_limit: u64)
    -> Transaction;

pub fn build_call(from: Address, contract: Address, input: Vec<u8>, value: u128, nonce: u64, gas_limit: u64)
    -> Transaction;

pub fn build_validator_update(from: Address, update: ValidatorUpdate, nonce: u64, gas_limit: u64)
    -> Transaction;

pub fn sign_tx(tx: &Transaction, key: &Ed25519PrivateKey) -> SignedTransaction;

// src/cli/keys.rs

pub fn generate_key_pair() -> (Ed25519PublicKey, Ed25519PrivateKey);
pub fn load_key_file(path: &Path) -> Result<(Ed25519PublicKey, Ed25519PrivateKey), CliError>;
pub fn derive_address(public_key: &Ed25519PublicKey) -> Address;
```

## 7. Internal Algorithms

### Build and Submit Transaction
```
async fn broadcast_tx(tx_args, key_file, wait, client):
    key = load_key_file(key_file)?
    tx  = build_tx_from_args(tx_args)?
    signed = sign_tx(&tx, &key.private_key)
    encoded = hex::encode(canonical_encode(&signed))

    if wait:
        result = client.call("broadcast_tx_commit", { tx: encoded, timeout_ms: 30000 }).await?
        print_commit_result(result)
    else:
        result = client.call("broadcast_tx", { tx: encoded }).await?
        print_broadcast_result(result)
```

### ABI Encoding (minimal)
```
fn encode_call_input(function_sig: &str, args_hex: &str) -> Vec<u8>:
    selector = sha256(function_sig.as_bytes())[0..4]
    args = hex::decode(args_hex)?
    [selector, args].concat()

// Example: encode_call_input("increment()", "") → [sha256("increment()")[0..4]]
// Example: encode_call_input("set(uint64)", hex(42u64)) → [selector, 0,0,0,0,0,0,0,42]
```

### Contract Address from Deploy Receipt
```
fn extract_contract_address(receipt: &Receipt) -> Option<Address>:
    // The state machine includes the deployed contract address in the receipt logs
    // Log format: topic="contract_deployed", data=address_bytes
    for log in receipt.logs:
        if log.topic == "contract_deployed":
            return Some(Address::from_bytes(&log.data))
    None
```

### Key Generation
```
fn generate_key_pair() -> (Ed25519PublicKey, Ed25519PrivateKey):
    keypair = Ed25519KeyPair::generate(&mut OsRng)
    (keypair.public_key(), keypair.private_key())

fn derive_address(pubkey) -> Address:
    // First 20 bytes of sha256(pubkey)
    Address::from_bytes(&sha256(pubkey.as_bytes())[0..20])
```

## 8. Persistence Model

The CLI is stateless across invocations. It reads key files and config files from disk on each run. It does not maintain a wallet database or nonce cache. Nonces must be supplied explicitly by the caller. A future enhancement could auto-fetch the nonce from the node via `get_account`.

## 9. Concurrency Model

The CLI is a single-threaded command-line tool. It uses `tokio::main` only for the `reqwest` HTTP client's async operations, which internally run on a single-thread runtime. There is no parallelism.

## 10. Configuration

```toml
# ~/.rustbft/config.toml (optional, all values overridable via flags)

[cli]
default_node = "http://localhost:26657"
default_output = "text"
default_gas_limit_transfer = 21000
default_gas_limit_deploy = 1000000
default_gas_limit_call = 100000
broadcast_tx_commit_timeout_ms = 30000
```

## 11. Observability

The CLI does not emit Prometheus metrics. It writes results to stdout and errors to stderr. Exit code 0 on success, 1 on error. All error messages include context: `"Error: failed to connect to node at http://localhost:26657: connection refused"`.

When `--output json` is set, all output including errors is JSON:
```json
{ "error": "connection refused", "node": "http://localhost:26657" }
```

## 12. Testing Strategy

- **`test_transfer_tx_encoding`**: build a transfer with known fields, canonically encode, hash → hash matches expected
- **`test_sign_tx_verifiable`**: sign a transaction with a test key, verify with the corresponding public key → valid
- **`test_deploy_tx_with_wasm`**: load a minimal valid WASM file, build deploy tx → code field contains file bytes
- **`test_call_tx_abi_encoding`**: `encode_call_input("increment()", "")` → 4-byte selector matches `sha256("increment()")[0..4]`
- **`test_generate_key_pair`**: generate key pair → public key is valid Ed25519, private key produces valid signatures
- **`test_load_key_file`**: write a known key JSON file, load it → public and private key bytes match expected
- **`test_derive_address_deterministic`**: same public key always produces same address
- **`test_rpc_client_broadcast_tx`**: mock HTTP server returns accepted → CLI prints tx_hash to stdout, exits 0
- **`test_rpc_client_broadcast_tx_rejected`**: mock server returns ERR_TX_REJECTED → CLI prints error to stderr, exits 1
- **`test_rpc_client_get_account`**: mock server returns account JSON → CLI prints balance and nonce
- **`test_rpc_client_not_found`**: mock server returns ERR_NOT_FOUND → CLI prints "not found", exits 1
- **`test_rpc_client_connection_refused`**: no server running → CLI prints connection error, exits 1
- **`test_output_json_format`**: run with `--output json`, capture stdout → valid JSON with expected keys
- **`test_wait_flag_polls_commit`**: `--wait` flag with mock server that fires commit notification → CLI waits and prints receipt

## 13. Open Questions

- **Nonce auto-fetch**: Requiring the operator to supply `--nonce` is error-prone. The CLI could call `get_account` automatically before building the transaction to fetch the current nonce. Deferred post-MVP.
- **ABI codec**: The current encoding is `selector + raw hex args`. A proper ABI codec (Solidity-compatible or custom) would allow calling contracts with typed arguments like `--input "increment()"` → automatically encode to bytes. Deferred post-MVP; raw hex is sufficient for testing.
