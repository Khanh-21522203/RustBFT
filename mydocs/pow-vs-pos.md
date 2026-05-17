# PoW vs PoS

## Proof of Work

Proof of Work is based on computation.

```text
Who creates blocks?
  Miners

How do they earn the right?
  By spending compute power to solve a hash puzzle.

Security comes from:
  Electricity and hardware cost.

Block selection:
  The first valid miner who finds the puzzle solution wins.

Typical examples:
  Bitcoin, old Ethereum.
```

The short version:

```text
PoW means:
  "Prove you spent computation."
```

## Proof of Stake

Proof of Stake is based on validator stake or voting power.

```text
Who creates/finalizes blocks?
  Validators

How do they earn the right?
  By having stake or voting power.

Security comes from:
  Economic stake that can be rewarded or slashed.

Block selection:
  A validator is selected as proposer, then validators vote.

Typical examples:
  modern Ethereum, Cosmos/Tendermint chains.
```

The short version:

```text
PoS means:
  "Prove you have voting power or stake at risk."
```

## Difference

```text
+----------------------+-----------------------------+-----------------------------+
| Topic                | PoW                         | PoS                         |
+----------------------+-----------------------------+-----------------------------+
| Main actor           | Miners                      | Validators                  |
| Right to propose     | Hash puzzle winner          | Selected validator          |
| Security source      | Compute/electricity cost    | Stake/voting power          |
| Finality style       | Usually probabilistic       | Often deterministic/BFT     |
| Energy use           | High                        | Lower                       |
| Voting weight        | Hash power                  | Stake or validator power    |
+----------------------+-----------------------------+-----------------------------+
```

## How RustBFT Acts Like PoS

RustBFT is not Proof of Work. It has:

```text
no mining
no hash puzzle
no nonce search
no difficulty adjustment
```

Instead, RustBFT uses a validator set:

```text
Validator {
  id,
  voting_power,
}
```

That makes the project PoS-like. Validators do not mine blocks. They propose
and vote. A block becomes final when validators with more than two thirds of
the total voting power agree on the same block.

RustBFT does not yet implement full staking economics:

```text
not yet:
  staking deposits
  delegation
  slashing
  rewards
  bonding/unbonding lifecycle
```

So the best description is:

```text
RustBFT is validator-voting BFT consensus.

It is PoS-like because voting power belongs to validators,
but it is not a complete staking/economic PoS system yet.
```

## RustBFT PoS-Like Flow

```text
                      +----------------------+
                      |    Validator Set     |
                      |                      |
                      |  A: voting_power 10  |
                      |  B: voting_power 20  |
                      |  C: voting_power 30  |
                      |  D: voting_power 40  |
                      +----------+-----------+
                                 |
                                 | proposer selected by
                                 | validator set / voting power
                                 v
                          +-------------+
                          |  Proposer   |
                          | creates     |
                          | block       |
                          +------+------+
                                 |
                                 | broadcasts proposal
                                 v

        -----------------------------------------------------
        |                    BFT Voting                     |
        -----------------------------------------------------

          +-------------+   +-------------+   +-------------+
          | Validator A |   | Validator B |   | Validator C |
          | power 10    |   | power 20    |   | power 30    |
          | prevote     |   | prevote     |   | prevote     |
          +------+------+   +------+------+   +------+------+
                 |                 |                 |
                 v                 v                 v
              +------------------------------------------+
              |        Collect prevotes by power         |
              |                                          |
              |        Need >2/3 total voting power      |
              +--------------------+---------------------+
                                   |
                                   v
          +-------------+   +-------------+   +-------------+
          | Validator A |   | Validator B |   | Validator C |
          | power 10    |   | power 20    |   | power 30    |
          | precommit   |   | precommit   |   | precommit   |
          +------+------+   +------+------+   +------+------+
                 |                 |                 |
                 v                 v                 v
              +------------------------------------------+
              |       Collect precommits by power        |
              |                                          |
              |       If >2/3 power signs same block     |
              |       block is committed/final           |
              +--------------------+---------------------+
                                   |
                                   v
                            +-------------+
                            |   Commit    |
                            | execute and |
                            | persist     |
                            +-------------+
```

## Code Map

```text
crates/core/src/validator.rs
  Validator
  ValidatorSet
  voting_power

crates/consensus/src/vote_set.rs
  sums voting power
  checks >2/3 quorum

crates/consensus/src/proposer.rs
  selects proposer from validator set

crates/consensus/src/state.rs
  runs propose -> prevote -> precommit -> commit
```

The key PoS-like rule is:

```text
1 validator does not always equal 1 vote.

validator influence = voting_power
```

If total voting power is `100`, finality needs:

```text
>2/3 of 100 = at least 67 voting power
```

That does not necessarily mean 67 validators. It means enough validators whose
combined voting power reaches the quorum threshold.
