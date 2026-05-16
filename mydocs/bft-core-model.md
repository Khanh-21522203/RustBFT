# BFT Core Model

## What BFT Means

BFT means Byzantine Fault Tolerance.

A Byzantine fault is worse than a normal crash. A faulty node may lie, send
conflicting messages, delay messages, or be controlled by an attacker. A BFT
consensus system lets honest validators agree on one canonical block even when
some validators are faulty or malicious.

The common safety threshold is:

```text
n validators can tolerate f Byzantine validators if:

n >= 3f + 1

Examples:
4 validators tolerate 1 faulty validator
7 validators tolerate 2 faulty validators
10 validators tolerate 3 faulty validators
```

RustBFT needs BFT because it is designed around multiple validators. No single
validator should be trusted to decide the chain alone. Validators must agree on
the same committed block at each height, and once a block is committed it should
have deterministic finality: no forks and no reorgs.

## Protocol Surfaces

This project uses a Tendermint-style BFT consensus protocol:

```text
Propose -> Prevote -> Precommit -> Commit
```

Validator-to-validator networking uses custom raw TCP, not gRPC:

```text
TCP socket
  -> plaintext handshake
  -> X25519 key exchange
  -> ChaCha20-Poly1305 encrypted frames
  -> custom message codec
  -> gossip proposal/vote messages
```

Client-to-node traffic uses HTTP JSON-RPC:

```text
Client -> HTTP JSON-RPC -> node
```

So the two main protocol surfaces are:

```text
Validator-to-validator: raw TCP custom encrypted P2P
Client-to-node:        HTTP JSON-RPC
```

## Core Model

```text
                         CLIENTS
                            |
                            | HTTP JSON-RPC
                            v
                    +----------------+
                    |      RPC       |
                    | broadcast_tx   |
                    | get_block      |
                    | get_account    |
                    +--------+-------+
                             |
                             | txs
                             v
                    +----------------+
                    |    Mempool     |
                    |  currently MVP |
                    +--------+-------+
                             |
                             | ReapTxs
                             v

+-------------------------------------------------------------------+
|                           NODE PROCESS                            |
|                                                                   |
|   +----------------+        commands        +----------------+    |
|   |                |----------------------->|                |    |
|   | Consensus Core |                        | Command Router |    |
|   |                |<-----------------------|                |    |
|   +--------+-------+        events          +---+--------+---+    |
|            |                                    |        |        |
|            |                                    |        |        |
|            v                                    v        v        |
|   +----------------+                    +----------+ +---------+  |
|   | Timer Service  |                    | Storage  | |Executor |  |
|   | timeouts       |                    | redb/WAL | |State VM |  |
|   +----------------+                    +----------+ +---------+  |
|                                                                   |
+-------------------------------------------------------------------+

                             ^
                             |
                             | proposals / prevotes / precommits
                             |
                    +--------+--------+
                    |       P2P       |
                    | raw TCP gossip  |
                    +--------+--------+
                             |
       ------------------------------------------------
       |                      |                       |
       v                      v                       v
+-------------+        +-------------+         +-------------+
| Validator A |        | Validator B |         | Validator C |
+-------------+        +-------------+         +-------------+
```

## Consensus Round

```text
Height H, Round R

        +----------+
        | Propose  |
        | proposer |
        | sends    |
        | block    |
        +----+-----+
             |
             v
        +----------+
        | Prevote  |
        | validators vote:
        | block or nil
        +----+-----+
             |
             | >2/3 prevotes
             v
        +-----------+
        | Precommit |
        | validators vote:
        | commit block or nil
        +----+------+
             |
             | >2/3 precommits
             v
        +----------+
        | Commit   |
        | execute  |
        | persist  |
        +----------+
```

## Mental Model

```text
core types       = block, vote, proposal, validator, tx, hash
consensus        = decide which block is final
p2p              = move proposals/votes between validators
execution        = apply transactions to app state
storage          = remember committed blocks/state
rpc              = let users query or submit txs
metrics          = observe the node
```

The important boundary is:

```text
Consensus does not execute business logic directly.

Consensus decides:
  "This block is committed."

Router then tells execution/storage:
  "Execute and persist this committed block."
```
