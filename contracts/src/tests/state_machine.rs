// SPDX-License-Identifier: MIT
//! Legal and illegal round state-machine transitions for both game modes.
//!
//! Issues #535 and #551 require the policy and phase matrices to be explicit,
//! executable, and aligned with the entrypoints in `contract.rs` and
//! `settlement.rs`. The tables below mirror `ROUND_LIFECYCLE.md` and
//! `PROTOCOL_SPEC.md`; tests assert the public status and error contracts so a
//! future guard cannot silently drift from the documented lifecycle.
//!
//! ## Round transition table
//!
//! | From                  | Event                     | To              | Modes covered |
//! |-----------------------|---------------------------|-----------------|---------------|
//! | `Unknown`             | `create_round`            | `Betting`       | Up/Down, Precision |
//! | `Betting`             | ledger reaches bet close  | `Running`       | Up/Down, Precision |
//! | `Running`             | ledger reaches end        | `AwaitingResolve` | Up/Down, Precision |
//! | `AwaitingResolve`     | successful `resolve_round` | `Resolved`     | Up/Down, Precision |
//! | `AwaitingResolve`     | minimum not met           | `FallbackRefund` | Up/Down, Precision |
//! | `Betting`/`Running`/`AwaitingResolve` | `cancel_round` | `Cancelled` | Up/Down, Precision |
//!
//! Terminal states are not active. Attempts to mutate or settle them must fail
//! with the entrypoint's specific error rather than a generic success.

use crate::contract::{VirtualTokenContract, VirtualTokenContractClient};
use crate::errors::ContractError;
use crate::types::{
    BetSide, OraclePayload, Round, RoundArchiveStatus, RoundMode, RoundPhase, RoundStatus,
};
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Bytes, BytesN, Env,
};

const START_LEDGER: u32 = 100;
const BETTING_LEDGER: u32 = START_LEDGER;
const RUNNING_LEDGER: u32 = START_LEDGER + 6;
const AWAITING_RESOLVE_LEDGER: u32 = START_LEDGER + 12;
const START_TIMESTAMP: u64 = 1_000;
const START_PRICE: u128 = 10_000;

fn set_ledger(env: &Env, sequence_number: u32) {
    env.ledger().with_mut(|ledger| {
        ledger.sequence_number = sequence_number;
        ledger.timestamp = START_TIMESTAMP;
    });
}

fn setup(env: &Env) -> (VirtualTokenContractClient<'_>, Address, Address) {
    let contract_id = env.register(VirtualTokenContract, ());
    let client = VirtualTokenContractClient::new(env, &contract_id);
    let admin = Address::generate(env);
    let oracle = Address::generate(env);
    env.mock_all_auths();
    client.initialize(&admin, &oracle);
    client.update_oracle_heartbeat(&0);
    (client, contract_id, admin)
}

fn create_round(env: &Env, client: &VirtualTokenContractClient, mode: u32) -> Round {
    client.create_round(&START_PRICE, &Some(mode));
    client.get_active_round().expect("create_round should store an active round")
}

fn oracle_payload(
    env: &Env,
    client: &VirtualTokenContractClient,
    round: &Round,
    price: u128,
) -> OraclePayload {
    OraclePayload {
        price,
        timestamp: env.ledger().timestamp(),
        round_id: round.start_ledger,
        nonce: round.round_id + 1,
        network_id: env.ledger().network_id(),
        contract_addr: client.address.clone(),
        confidence: None,
        attestation: None,
    }
}

fn valid_commitment(
    env: &Env,
    predicted_price: u128,
) -> (BytesN<32>, BytesN<32>) {
    let salt = BytesN::from_array(
        env,
        &[
            0x42, 0x43, 0x43, 0x43, 0x43, 0x43, 0x43, 0x43, 0x43, 0x43, 0x43, 0x43, 0x43,
            0x43, 0x43, 0x43, 0x43, 0x43, 0x43, 0x43, 0x43, 0x43, 0x43, 0x43, 0x43, 0x43,
            0x43, 0x43, 0x43, 0x43, 0x43, 0x44,
        ],
    );
    let mut preimage = Bytes::new(env);
    preimage.append(&predicted_price.to_xdr(env));
    preimage.append(&salt.to_xdr(env));
    let hash: BytesN<32> = env.crypto().sha256(&preimage).into();
    (hash, salt)
}

fn participate(env: &Env, client: &VirtualTokenContractClient, mode: u32) -> Address {
    let user = Address::generate(env);
    client.mint_initial(&user);
    match mode {
        0 => client.place_bet(&user, &100_000000, &BetSide::Up),
        1 => client.place_precision_prediction(&user, &100_000000, &12_500),
        _ => unreachable!("test helper accepts only Up/Down and Precision"),
    }
    user
}

fn assert_derived_phase_transitions(env: &Env, mode: u32) {
    set_ledger(env, BETTING_LEDGER);
    let (client, _contract_id, _admin) = setup(env);
    let round = create_round(env, &client, mode);

    assert_eq!(client.get_round_phase(), Ok(RoundPhase::Betting));
    assert_eq!(
        client.get_round_status(round.round_id),
        RoundStatus::Betting
    );

    set_ledger(env, RUNNING_LEDGER);
    assert_eq!(client.get_round_phase(), Ok(RoundPhase::Running));
    assert_eq!(
        client.get_round_status(round.round_id),
        RoundStatus::Running
    );

    set_ledger(env, AWAITING_RESOLVE_LEDGER);
    assert_eq!(client.get_round_phase(), Ok(RoundPhase::Resolvable));
    assert_eq!(
        client.get_round_status(round.round_id),
        RoundStatus::AwaitingResolve
    );
}

#[test]
fn test_updown_derived_phase_transitions() {
    let env = Env::default();
    assert_derived_phase_transitions(&env, 0);
}

#[test]
fn test_precision_derived_phase_transitions() {
    let env = Env::default();
    assert_derived_phase_transitions(&env, 1);
}

fn assert_cancellation(env: &Env, mode: u32, phase_ledger: u32) {
    set_ledger(env, phase_ledger);
    let (client, _contract_id, _admin) = setup(env);
    let round = create_round(env, &client, mode);
    let user = participate(env, &client, mode);

    client.cancel_round(&0);
    assert!(client.get_active_round().is_none());
    assert_eq!(
        client.get_round_status(round.round_id),
        RoundStatus::Cancelled
    );
    let archive = client
        .get_archived_round(round.round_id)
        .expect("cancelled round should be archived");
    assert_eq!(archive.status, RoundArchiveStatus::Cancelled);
    assert_eq!(
        archive.mode,
        if mode == 0 {
            RoundMode::UpDown
        } else {
            RoundMode::Precision
        }
    );

    // A terminal round cannot be settled or cancelled again.
    assert_eq!(
        client.try_resolve_round(&oracle_payload(env, &client, &round, 11_000)),
        Err(Ok(ContractError::NoActiveRound))
    );
    assert_eq!(
        client.try_cancel_round(&0),
        Err(Ok(ContractError::RoundNotCancellable))
    );

    // A replacement cannot reuse the same start ledger, but is legal once the
    // ledger advances.
    assert_eq!(
        client.try_create_round(&START_PRICE, &Some(mode)),
        Err(Ok(ContractError::RoundStartLedgerReused))
    );
    set_ledger(env, phase_ledger + 1);
    let replacement = create_round(env, &client, mode);
    assert_eq!(
        client.get_round_status(replacement.round_id),
        RoundStatus::Betting
    );
    assert_eq!(client.balance(&user), 900_000000);
}

#[test]
fn test_updown_can_cancel_from_every_active_phase() {
    for phase_ledger in [BETTING_LEDGER, RUNNING_LEDGER, AWAITING_RESOLVE_LEDGER] {
        let env = Env::default();
        assert_cancellation(&env, 0, phase_ledger);
    }
}

#[test]
fn test_precision_can_cancel_from_every_active_phase() {
    for phase_ledger in [BETTING_LEDGER, RUNNING_LEDGER, AWAITING_RESOLVE_LEDGER] {
        let env = Env::default();
        assert_cancellation(&env, 1, phase_ledger);
    }
}

fn assert_resolution(env: &Env, mode: u32) {
    set_ledger(env, BETTING_LEDGER);
    let (client, _contract_id, _admin) = setup(env);
    let round = create_round(env, &client, mode);
    let _user = participate(env, &client, mode);
    set_ledger(env, AWAITING_RESOLVE_LEDGER);

    let payload = oracle_payload(env, &client, &round, 11_000);
    client.resolve_round(&payload);

    assert!(client.get_active_round().is_none());
    assert_eq!(
        client.get_round_status(round.round_id),
        RoundStatus::Resolved
    );
    let archive = client
        .get_archived_round(round.round_id)
        .expect("resolved round should be archived");
    assert_eq!(archive.status, RoundArchiveStatus::Resolved);
    assert_eq!(
        archive.mode,
        if mode == 0 {
            RoundMode::UpDown
        } else {
            RoundMode::Precision
        }
    );

    assert_eq!(
        client.try_cancel_round(&0),
        Err(Ok(ContractError::RoundNotCancellable))
    );
    assert_eq!(
        client.try_resolve_round(&payload),
        Err(Ok(ContractError::NoActiveRound))
    );
}

#[test]
fn test_updown_resolves_from_awaiting_resolve() {
    let env = Env::default();
    assert_resolution(&env, 0);
}

#[test]
fn test_precision_resolves_from_awaiting_resolve() {
    let env = Env::default();
    assert_resolution(&env, 1);
}

fn assert_fallback_resolution(env: &Env, mode: u32) {
    set_ledger(env, BETTING_LEDGER);
    let (client, _contract_id, _admin) = setup(env);
    client.set_min_participants(&Some(2));
    let round = create_round(env, &client, mode);
    let _user = participate(env, &client, mode);
    set_ledger(env, AWAITING_RESOLVE_LEDGER);

    client.resolve_round(&oracle_payload(env, &client, &round, 11_000));

    assert_eq!(
        client.get_round_status(round.round_id),
        RoundStatus::FallbackRefund
    );
    let archive = client
        .get_archived_round(round.round_id)
        .expect("fallback round should be archived");
    assert_eq!(archive.status, RoundArchiveStatus::FallbackRefund);
    assert_eq!(
        archive.mode,
        if mode == 0 {
            RoundMode::UpDown
        } else {
            RoundMode::Precision
        }
    );
}

#[test]
fn test_updown_can_take_fallback_refund_transition() {
    let env = Env::default();
    assert_fallback_resolution(&env, 0);
}

#[test]
fn test_precision_can_take_fallback_refund_transition() {
    let env = Env::default();
    assert_fallback_resolution(&env, 1);
}

fn assert_premature_resolution(env: &Env, mode: u32) {
    set_ledger(env, BETTING_LEDGER);
    let (client, _contract_id, _admin) = setup(env);
    let round = create_round(env, &client, mode);
    let payload = oracle_payload(env, &client, &round, 11_000);

    assert_eq!(
        client.try_resolve_round(&payload),
        Err(Ok(ContractError::RoundNotEnded))
    );
    assert_eq!(
        client.get_round_status(round.round_id),
        RoundStatus::Betting
    );

    set_ledger(env, RUNNING_LEDGER);
    assert_eq!(
        client.try_resolve_round(&payload),
        Err(Ok(ContractError::RoundNotEnded))
    );
    assert_eq!(
        client.get_round_status(round.round_id),
        RoundStatus::Running
    );
}

#[test]
fn test_updown_rejects_premature_resolution_in_betting_and_running() {
    let env = Env::default();
    assert_premature_resolution(&env, 0);
}

#[test]
fn test_precision_rejects_premature_resolution_in_betting_and_running() {
    let env = Env::default();
    assert_premature_resolution(&env, 1);
}

#[test]
fn test_updown_rejects_participation_after_betting_closes() {
    let env = Env::default();
    set_ledger(&env, BETTING_LEDGER);
    let (client, _contract_id, _admin) = setup(&env);
    create_round(&env, &client, 0);
    let user = Address::generate(&env);
    client.mint_initial(&user);
    set_ledger(&env, RUNNING_LEDGER);

    assert_eq!(
        client.try_place_bet(&user, &100_000000, &BetSide::Down),
        Err(Ok(ContractError::RoundEnded))
    );
    assert_eq!(client.balance(&user), 1000_000000);
}

#[test]
fn test_precision_rejects_participation_after_betting_closes() {
    let env = Env::default();
    set_ledger(&env, BETTING_LEDGER);
    let (client, _contract_id, _admin) = setup(&env);
    create_round(&env, &client, 1);
    let user = Address::generate(&env);
    client.mint_initial(&user);
    set_ledger(&env, RUNNING_LEDGER);

    assert_eq!(
        client.try_place_precision_prediction(&user, &100_000000, &12_500),
        Err(Ok(ContractError::RoundEnded))
    );
    assert_eq!(
        client.try_commit_prediction(
            &user,
            &BytesN::from_array(&env, &[1; 32]),
            &100_000000
        ),
        Err(Ok(ContractError::RoundEnded))
    );
    assert_eq!(client.balance(&user), 1000_000000);
}

fn assert_reveal_window(env: &Env, reveal_ledger: u32) {
    set_ledger(env, BETTING_LEDGER);
    let (client, _contract_id, _admin) = setup(env);
    create_round(env, &client, 1);
    let user = Address::generate(env);
    client.mint_initial(&user);
    let (hash, salt) = valid_commitment(env, 12_500);
    client.commit_prediction(&user, &hash, &100_000000);
    set_ledger(env, reveal_ledger);

    assert_eq!(
        client.try_reveal_prediction(&user, &12_500, &salt),
        Err(Ok(ContractError::InvalidRevealWindow))
    );
    assert_eq!(client.balance(&user), 900_000000);
}

#[test]
fn test_precision_rejects_reveal_in_betting_and_awaiting_resolve() {
    let env = Env::default();
    assert_reveal_window(&env, BETTING_LEDGER);
    let env = Env::default();
    assert_reveal_window(&env, AWAITING_RESOLVE_LEDGER);
}

#[test]
fn test_precision_allows_reveal_in_running_phase() {
    let env = Env::default();
    set_ledger(&env, BETTING_LEDGER);
    let (client, _contract_id, _admin) = setup(&env);
    create_round(&env, &client, 1);
    let user = Address::generate(&env);
    client.mint_initial(&user);
    let (hash, salt) = valid_commitment(&env, 12_500);
    client.commit_prediction(&user, &hash, &100_000000);
    set_ledger(&env, RUNNING_LEDGER);

    client.reveal_prediction(&user, &12_500, &salt);
    let prediction = client
        .get_user_precision_prediction(&user)
        .expect("reveal should create the precision position");
    assert_eq!(prediction.predicted_price, 12_500);
    assert_eq!(prediction.amount, 100_000000);
}

#[test]
fn test_mode_isolation_rejects_cross_mode_entrypoints() {
    let env = Env::default();
    set_ledger(&env, BETTING_LEDGER);
    let (client, _contract_id, _admin) = setup(&env);
    create_round(&env, &client, 0);
    let user = Address::generate(&env);
    client.mint_initial(&user);
    let (hash, salt) = valid_commitment(&env, 12_500);

    assert_eq!(
        client.try_place_precision_prediction(&user, &100_000000, &12_500),
        Err(Ok(ContractError::WrongModeForPrediction))
    );
    assert_eq!(
        client.try_commit_prediction(&user, &hash, &100_000000),
        Err(Ok(ContractError::WrongModeForPrediction))
    );
    assert_eq!(
        client.try_reveal_prediction(&user, &12_500, &salt),
        Err(Ok(ContractError::WrongModeForPrediction))
    );

    let env = Env::default();
    set_ledger(&env, BETTING_LEDGER);
    let (client, _contract_id, _admin) = setup(&env);
    create_round(&env, &client, 1);
    let user = Address::generate(&env);
    client.mint_initial(&user);
    assert_eq!(
        client.try_place_bet(&user, &100_000000, &BetSide::Up),
        Err(Ok(ContractError::WrongModeForPrediction))
    );
    assert_eq!(client.balance(&user), 1000_000000);
}

#[test]
fn test_creating_a_second_active_round_is_rejected_in_both_modes() {
    for mode in [0, 1] {
        let env = Env::default();
        set_ledger(&env, BETTING_LEDGER);
        let (client, _contract_id, _admin) = setup(&env);
        let first = create_round(&env, &client, mode);

        assert_eq!(
            client.try_create_round(&START_PRICE, &Some(1 - mode)),
            Err(Ok(ContractError::RoundAlreadyActive))
        );
        assert_eq!(
            client.get_round_status(first.round_id),
            RoundStatus::Betting
        );
    }
}
