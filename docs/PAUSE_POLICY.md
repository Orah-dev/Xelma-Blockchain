# Pause Policy Matrix (`RuntimeMode` × `PolicyAction`)

Canonical reference for **which contract actions are allowed in which runtime
mode**. It documents what `admin::_policy_gate` actually implements, and names
the test that enforces every cell so the table cannot silently drift.

- Implementation: `contracts/src/admin.rs` → `_policy_gate`, `_ensure_not_paused`, `_ensure_normal_mode`
- Types: `contracts/src/types.rs` → `RuntimeMode`, `PolicyAction`
- Enforcement tests: `contracts/src/tests/pause_policy_matrix.rs`, `contracts/src/tests/policy_gate.rs`
- Operational runbook: [EMERGENCY_DRILL.md](./EMERGENCY_DRILL.md)

---

## 1. Runtime modes

Stored under `DataKeyCore::Paused` as a `RuntimeMode`; an unset key reads as
`Normal`. Read it with `get_runtime_mode()`.

| value | variant | meaning |
|---|---|---|
| 0 | `Normal` | Everything the protocol normally allows. |
| 1 | `ClaimsOnly` | Incident mode: no new risk is taken, but in-flight rounds and withdrawals keep working. |
| 2 | `FullyPaused` | Emergency stop. Every gated action is rejected. |

Mode transitions:

| call | effect |
|---|---|
| `set_runtime_mode(mode)` | Sets any mode directly (admin). Never gated — see §5. |
| `pause_contract()` | `→ FullyPaused`. |
| `unpause_contract()` | `FullyPaused → Normal` when an active round exists, otherwise `→ ClaimsOnly`. |

Helpers: `is_paused()` is exactly `mode == FullyPaused`; `get_protocol_health()`
reports `status_code` (`6` = claims-only, `1` = paused);
`is_action_allowed(action)` answers the table below without mutating state.

---

## 2. The matrix

| action class | `Normal` | `ClaimsOnly` | `FullyPaused` |
|---|:---:|:---:|:---:|
| `RoundMutation` | ✅ | ❌ | ❌ |
| `Claim` | ✅ | ✅ | ❌ |
| `Settlement` | ✅ | ✅ | ❌ |
| `AdminConfig` | ✅ | ✅ | ❌ |

`RoundMutation` is the **only** class `ClaimsOnly` blocks. Stopping it is the
point of the mode: once the protocol has decided to take no new risk, no new
bets, commits, reveals, mints, or cash-outs may enter, while an in-flight round
can still be settled/cancelled/voided and winners can still withdraw. Everything
else is blocked only by `FullyPaused`.

The whole matrix is these two rules in `_policy_gate`:

```rust
PolicyAction::RoundMutation => mode != RuntimeMode::Normal,
PolicyAction::Claim | PolicyAction::AdminConfig | PolicyAction::Settlement
    => mode == RuntimeMode::FullyPaused,
```

A blocked action returns `ContractError::ContractPaused` (error `22`).

---

## 3. Entrypoint inventory

Every public entrypoint that is gated, by class.

### 3.1 `RoundMutation` — blocked in `ClaimsOnly` and `FullyPaused`

| entrypoint | gate call |
|---|---|
| `place_bet` | `_ensure_normal_mode` |
| `place_precision_prediction` | `_ensure_normal_mode` |
| `predict_price` | `_ensure_normal_mode` (delegates to `place_precision_prediction`) |
| `commit_prediction` | `_ensure_normal_mode` |
| `reveal_prediction` | `_ensure_normal_mode` |
| `cash_out_early` | `_ensure_normal_mode` |
| `mint_initial` | `_ensure_normal_mode` |
| `apply_scheduled_changes` | `_ensure_normal_mode` |

`apply_scheduled_changes` is the deliberate odd one out in the config surface:
activating a timelocked change is treated as mutation-adjacent, so an incident
freezes **pending config activations** along with new bets. Cancelling a pending
change is *not* — `cancel_config_change` is `AdminConfig`, so an operator can
always back a scheduled change out even while `ClaimsOnly`.

### 3.2 `Claim` — blocked only in `FullyPaused`

| entrypoint | gate call |
|---|---|
| `claim_winnings` | `_ensure_not_paused` |
| `claim_many` | `_ensure_not_paused` |

Both are `_ensure_not_paused` rather than `_policy_gate(env, PolicyAction::Claim)`;
`_ensure_not_paused` is defined as `_policy_gate(env, PolicyAction::AdminConfig)`,
which blocks on exactly the same condition (`FullyPaused`). The blocking
behaviour is identical — `Claim` is the semantic label, not a distinct rule.
**This is the acceptance criterion for `ClaimsOnly`: claims always succeed
there, so a user can never be locked out of winnings they already earned.**

### 3.3 `Settlement` — blocked only in `FullyPaused`

| entrypoint | gate call |
|---|---|
| `resolve_round` | `_ensure_not_paused` |
| `resolve_round_multi` | `_ensure_not_paused` |
| `cancel_round` | `_ensure_not_paused` |
| `void_round` | `_ensure_not_paused` |
| `finalize_round` | `_ensure_not_paused` |

An in-flight round can always be brought to a terminal state during an incident,
so funds are never stranded by a `ClaimsOnly` escalation.

### 3.4 `AdminConfig` — blocked only in `FullyPaused`

| area | entrypoints |
|---|---|
| Schema | `migrate_schema_v1_to_v2`, `migrate_schema_v2_to_v3` |
| Oracle config | `set_oracle_max_deviation_bps`, `set_deviation_ref_mode`, `set_attestation_key`, `arm_oracle_deviation_override`, `set_oracle_min_confidence_bps`, `set_oracle_strict_mode`, `set_hb_strict_mode`, `arm_hb_override`, `set_hb_grace_seconds`, `set_oracle_quorum_config`, `set_oracle_stale_threshold` |
| Oracle rotation | `propose_oracle_rotation`, `accept_oracle_rotation`, `cancel_oracle_rotation` |
| Access control | `set_access_control_enabled`, `add_allowlisted`, `remove_allowlisted`, `add_denylisted`, `remove_denylisted` |
| Windows / stakes | `set_windows`, `set_max_stake`, `set_max_user_exposure`, `set_max_pending_winnings`, `set_min_bet`, `set_pending_winnings_expiry`, `set_precision_payout_policy`, `set_min_participants`, `set_max_precision_participants`, `set_close_buffer_ledgers`, `set_early_cashout_bps`, `set_dispute_ledgers` |
| Minting | `set_mint_limit`, `set_epoch_mint_budget` |
| Fees / insurance | `set_protocol_fee_bps`, `withdraw_protocol_fee`, `set_fee_model`, `set_insurance_split_bps`, `set_insurance_coverage_bps`, `set_insurance_eligible_events`, `top_up_insurance_fund`, `withdraw_insurance_fund` |
| Timelock | `schedule_*` (all 10 variants), `cancel_config_change` |
| Round lifecycle | `create_round`, `create_next_from_template`, `set_round_template`, `clear_round_template` |
| Maintenance | `set_archive_retention`, `reclaim_expired_pending_winnings`, `batch_touch_ttl`, `reset_leaderboard_season` |

`create_round` / `create_next_from_template` are deliberately **not**
`RoundMutation`: they are the entrypoints that transition the protocol back out
of `ClaimsOnly` into `Active`, so they must stay callable in that mode.

---

## 4. Not gated, and why

| entrypoint | reason |
|---|---|
| `pause_contract`, `unpause_contract`, `set_runtime_mode` | They call `_set_mode` directly. They must work in every mode — `FullyPaused` included — or there is no way out of an incident. Still admin-authenticated. **Do not add a `_policy_gate` call to these.** |
| `update_oracle_heartbeat` | Oracle liveness must keep flowing during an incident so `get_protocol_health()` stays truthful. |
| `initialize` | One-shot; the contract cannot be paused before it exists. |
| all `get_*` / `is_*` queries, `balance`, `simulate_payout`, `batch_touch_ttl` readers | Read-only; must stay available so operators and users can inspect state. |
| `announce_next_schema`, `clear_next_schema` | **No stated reason** — these are admin writes to the migration-announcement key and carry no gate. Low impact (they only declare an upcoming schema version; the migrations themselves are `AdminConfig`-gated), but they are another place where `FullyPaused` is not a uniform stop. Flagged below with `cancel_round`. |

### Known gaps: documented classes are not fully gated

**`cancel_round` is listed under `Settlement` in §3.3 but has no gate at all.**
`settlement::cancel_round` contains no `_policy_gate` / `_ensure_not_paused`
call, so `FullyPaused` does not stop a cancellation. Cancelling only returns
stakes, so it takes no risk — but "the emergency stop does not stop this" is a
maintainer decision, not something the code should decide by accident. Pinned
by `test_cancel_round_is_ungated_and_diverges_from_the_matrix`.

**`announce_next_schema` and `clear_next_schema` have no gate.** They only
write the migration-announcement key, and the migrations they announce
(`migrate_schema_v1_to_v2`, `migrate_schema_v2_to_v3`) are `AdminConfig`-gated,
so the practical blast radius is small — but they are admin writes that survive
`FullyPaused`.

**The governance surface is not gated.** No function in
`contracts/src/governance.rs` calls `_policy_gate`, `_ensure_not_paused`, or
`_ensure_normal_mode`, so the following are callable in `FullyPaused`:

`set_gov_approver`, `set_gov_proposal_ttl`, `propose_gov_action`,
`approve_gov_proposal`, `execute_gov_proposal`, `cancel_gov_proposal`,
`establish_constitution`, `propose_amendment`, `veto_amendment`,
`activate_amendment`.

`execute_gov_proposal` is the one to look at: if a queued governance action can
reach a config write that does its own gating, the net effect is still the
config surface's policy; if it bypasses a gate its direct counterpart would
have hit, then `FullyPaused` is not a uniform stop.

**This table documents current behaviour; it is not a claim that the behaviour
is correct.** Both gaps are for maintainers to close (or accept) — per
`GOVERNANCE.md`, a security-sensitive change needs explicit core-maintainer
sign-off, so neither is fixed here.

---

## 5. Test coverage

Every cell in §2 is asserted by an executable test. Run them with:

```bash
cargo test --package xelma-contract --lib -- tests::policy_gate tests::pause_policy_matrix
cargo test --package xelma-contract --lib -- tests::drill
```

| cell | test |
|---|---|
| `Normal` allows all four classes | `policy_gate::test_policy_gate_normal_mode_allows_everything` |
| `ClaimsOnly` blocks only `RoundMutation` | `policy_gate::test_policy_gate_claims_only_blocks_only_round_mutation` |
| `FullyPaused` blocks all four classes | `policy_gate::test_policy_gate_fully_paused_blocks_everything` |
| the same, read back off-chain | `pause_policy_matrix::test_claims_only_does_not_block_the_rest_of_the_matrix` |
| `RoundMutation` — every entrypoint, `ClaimsOnly` | `pause_policy_matrix::test_claims_only_blocks_every_round_mutation_entrypoint` |
| `RoundMutation` — every entrypoint, `FullyPaused` | `pause_policy_matrix::test_fully_paused_blocks_every_round_mutation_entrypoint` |
| `Claim` — **pays out in `ClaimsOnly`**, denied in `FullyPaused` | `pause_policy_matrix::test_claims_only_allows_claims_and_fully_paused_denies_them` |
| `Settlement` — not blocked in `ClaimsOnly` | `pause_policy_matrix::test_claims_only_does_not_block_settlement` |
| `Settlement` — blocked in `FullyPaused` | `pause_policy_matrix::test_fully_paused_blocks_settlement` |
| `AdminConfig` — every entrypoint, both cells | `pause_policy_matrix::test_claims_only_does_not_block_any_admin_config_entrypoint`, `…::test_fully_paused_blocks_every_admin_config_entrypoint` |
| `apply_scheduled_changes` is `RoundMutation`, not `AdminConfig` | `pause_policy_matrix::test_apply_scheduled_changes_is_blocked_in_claims_only_and_fully_paused` |
| Mode controls never gate themselves | `pause_policy_matrix::test_set_runtime_mode_is_never_blocked_by_its_own_gate` |
| `cancel_round` divergence is pinned | `pause_policy_matrix::test_cancel_round_is_ungated_and_diverges_from_the_matrix` |
| Whole-matrix drill (real balances, real settlement, real claims) | `drill::test_claims_only_matrix_verification`, `drill::test_fully_paused_matrix_verification`, `drill::test_emergency_incident_simulation_lifecycle` |

The `Claim` row is the only one that has to prove money *moved*: it settles a
real round, escalates to `ClaimsOnly`, then asserts the winner's balance grows
by exactly the pending amount — not merely that the call did not return
`ContractPaused`.

The `ClaimsOnly` assertions are deliberately asymmetric: a call must **not**
return `ContractPaused`. It may still fail for an orthogonal reason (no pending
rotation, no active round, bad argument) — the property under test is that the
*policy gate* let it through, not that it fully succeeded.
