//! Pairing through the model: confirmation (numeric comparison and Just Works), passkey entry and
//! display, refusal, remote failure, timeout and a pairing the remote side starts.

use crate::common;

use common::*;
use cupertino_bluetooth::{
    Command, Eir, EirName, Event, IoCapability, KeyValue, LinkKey, LinkKeyType, Opcode, Status,
};
use cupertino_btd::device::DeviceRef;
use cupertino_btd::keys::KeySet;
use cupertino_btd::pairing::{AgentReply, Prompt, PAIRING_TIMEOUT};
use cupertino_btd::request::{AgentAnswer, Change, Effect, Request, RequestError, Ticket};

fn phone_ref() -> DeviceRef {
    DeviceRef {
        address: phone().address,
        kind: Some(phone().kind),
    }
}

/// A powered adapter that has discovered a phone named "Phone".
fn discovered() -> Harness {
    let mut h = Harness::powered();
    h.found(
        phone(),
        Some(-50),
        0,
        Eir {
            name: Some(EirName {
                text: "Phone".to_owned(),
                complete: true,
            }),
            ..Eir::default()
        },
    );
    h
}

/// Starts pairing with the phone and returns the request's ticket.
fn start_pairing(h: &mut Harness) -> Ticket {
    let (ticket, effects) = h.request(Request::Pair(phone_ref()));
    assert_eq!(
        sent(&effects),
        [Command::PairDevice {
            device: phone(),
            io_capability: IoCapability::KeyboardDisplay
        }]
    );
    assert!(completions(&effects).is_empty());
    ticket
}

fn link_key() -> LinkKey {
    LinkKey {
        device: phone(),
        key_type: LinkKeyType::AuthenticatedP256,
        value: KeyValue([0x42; 16]),
        pin_length: 0,
    }
}

#[test]
fn numeric_comparison_accepted_pairs_and_stores_the_key() {
    let mut h = discovered();
    let (ticket, effects) = h.request(Request::Pair(phone_ref()));
    // The device is marked as being paired.
    let Change::DeviceChanged(info) = only_change(&effects) else {
        panic!("expected a DeviceChanged")
    };
    assert!(info.pairing && !info.paired);

    let effects = h.packet(Event::UserConfirmationRequest {
        device: phone(),
        just_works: false,
        value: 123_456,
    });
    let calls = agent_calls(&effects);
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].prompt,
        Prompt::Confirm {
            passkey: 123_456,
            just_works: false
        }
    );
    assert_eq!(calls[0].device.name.as_deref(), Some("Phone"));

    let effects = h.answer(AgentAnswer {
        pairing: calls[0].pairing,
        reply: AgentReply::Confirm(true),
    });
    assert_eq!(
        sent(&effects),
        [Command::UserConfirmationReply {
            device: phone(),
            accept: true
        }]
    );

    // The kernel sends the key, then the reply to Pair Device.
    let effects = h.packet(Event::NewLinkKey {
        store: true,
        key: link_key(),
    });
    let mut expected = KeySet::default();
    expected.set_link_key(link_key());
    expected.set_name(&phone(), "Phone");
    assert!(effects.contains(&Effect::SaveKeys {
        address: ADAPTER,
        keys: expected
    }));
    assert!(completions(&effects).is_empty());

    let effects = h.reply(Opcode::PAIR_DEVICE, Status::Success, device_reply(phone()));
    assert_eq!(completions(&effects), [(ticket, Ok(()))]);
    let info = &h.model.devices()[0];
    assert_eq!(
        (info.paired, info.bonded, info.pairing),
        (true, true, false)
    );
}

#[test]
fn just_works_asks_a_plain_yes_or_no() {
    let mut h = discovered();
    start_pairing(&mut h);
    let effects = h.packet(Event::UserConfirmationRequest {
        device: phone(),
        just_works: true,
        value: 0,
    });
    assert_eq!(
        agent_calls(&effects)[0].prompt,
        Prompt::Confirm {
            passkey: 0,
            just_works: true
        }
    );
}

#[test]
fn a_refusal_fails_the_pairing_as_rejected() {
    let mut h = discovered();
    let ticket = start_pairing(&mut h);
    let effects = h.packet(Event::UserConfirmationRequest {
        device: phone(),
        just_works: false,
        value: 111_111,
    });
    let effects = h.answer(AgentAnswer {
        pairing: agent_calls(&effects)[0].pairing,
        reply: AgentReply::Confirm(false),
    });
    assert_eq!(
        sent(&effects),
        [Command::UserConfirmationReply {
            device: phone(),
            accept: false
        }]
    );

    // The kernel then reports the failed authentication and fails the command.
    assert!(completions(&h.packet(Event::AuthenticationFailed {
        device: phone(),
        status: Status::AuthenticationFailed
    }))
    .is_empty());
    let effects = h.reply(
        Opcode::PAIR_DEVICE,
        Status::AuthenticationFailed,
        device_reply(phone()),
    );
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::Rejected))]
    );
    assert!(!h.model.devices()[0].paired);
}

#[test]
fn the_remote_side_failing_the_authentication_is_reported_as_such() {
    let mut h = discovered();
    let ticket = start_pairing(&mut h);
    h.packet(Event::AuthenticationFailed {
        device: phone(),
        status: Status::AuthenticationFailed,
    });
    let effects = h.reply(
        Opcode::PAIR_DEVICE,
        Status::AuthenticationFailed,
        device_reply(phone()),
    );
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::AuthenticationFailed))]
    );
}

#[test]
fn other_failures_of_the_command_are_mapped_from_their_status() {
    let mut h = discovered();
    let ticket = start_pairing(&mut h);
    let effects = h.reply(
        Opcode::PAIR_DEVICE,
        Status::ConnectFailed,
        device_reply(phone()),
    );
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::ConnectFailed))]
    );

    let ticket = start_pairing(&mut h);
    let effects = h.reply(Opcode::PAIR_DEVICE, Status::Failed, device_reply(phone()));
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::Controller(Status::Failed)))]
    );
}

#[test]
fn a_passkey_the_user_types_is_sent_to_the_kernel() {
    let mut h = discovered();
    let ticket = start_pairing(&mut h);
    let effects = h.packet(Event::UserPasskeyRequest { device: phone() });
    let calls = agent_calls(&effects);
    assert_eq!(calls[0].prompt, Prompt::RequestPasskey);

    let effects = h.answer(AgentAnswer {
        pairing: calls[0].pairing,
        reply: AgentReply::Passkey(Some(482_913)),
    });
    assert_eq!(
        sent(&effects),
        [Command::UserPasskeyReply {
            device: phone(),
            passkey: Some(482_913)
        }]
    );
    h.packet(Event::NewLinkKey {
        store: true,
        key: link_key(),
    });
    let effects = h.reply(Opcode::PAIR_DEVICE, Status::Success, device_reply(phone()));
    assert_eq!(completions(&effects), [(ticket, Ok(()))]);
}

#[test]
fn a_passkey_the_user_declines_to_type_is_a_refusal() {
    let mut h = discovered();
    let ticket = start_pairing(&mut h);
    let effects = h.packet(Event::UserPasskeyRequest { device: phone() });
    let effects = h.answer(AgentAnswer {
        pairing: agent_calls(&effects)[0].pairing,
        reply: AgentReply::Passkey(None),
    });
    assert_eq!(
        sent(&effects),
        [Command::UserPasskeyReply {
            device: phone(),
            passkey: None
        }]
    );
    let effects = h.reply(
        Opcode::PAIR_DEVICE,
        Status::AuthenticationFailed,
        device_reply(phone()),
    );
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::Rejected))]
    );
}

#[test]
fn a_passkey_to_type_on_the_other_device_is_shown_with_its_progress() {
    let mut h = discovered();
    start_pairing(&mut h);
    let effects = h.packet(Event::PasskeyNotify {
        device: phone(),
        passkey: 654_321,
        entered: 0,
    });
    assert_eq!(
        agent_calls(&effects)[0].prompt,
        Prompt::DisplayPasskey {
            passkey: 654_321,
            entered: 0
        }
    );
    let effects = h.packet(Event::PasskeyNotify {
        device: phone(),
        passkey: 654_321,
        entered: 3,
    });
    assert_eq!(
        agent_calls(&effects)[0].prompt,
        Prompt::DisplayPasskey {
            passkey: 654_321,
            entered: 3
        }
    );
}

#[test]
fn no_agent_means_the_pairing_is_refused_and_says_why() {
    let mut h = discovered();
    let ticket = start_pairing(&mut h);
    let effects = h.packet(Event::UserConfirmationRequest {
        device: phone(),
        just_works: false,
        value: 1,
    });
    let effects = h.answer(AgentAnswer {
        pairing: agent_calls(&effects)[0].pairing,
        reply: AgentReply::Failed("no pairing agent is registered".to_owned()),
    });
    assert_eq!(
        sent(&effects),
        [Command::UserConfirmationReply {
            device: phone(),
            accept: false
        }]
    );
    let effects = h.reply(
        Opcode::PAIR_DEVICE,
        Status::AuthenticationFailed,
        device_reply(phone()),
    );
    assert_eq!(
        completions(&effects),
        [(
            ticket,
            Err(RequestError::Agent(
                "no pairing agent is registered".to_owned()
            ))
        )]
    );
}

#[test]
fn a_pairing_nobody_answers_is_cancelled_at_its_deadline() {
    let mut h = discovered();
    let ticket = start_pairing(&mut h);
    let effects = h.packet(Event::UserConfirmationRequest {
        device: phone(),
        just_works: false,
        value: 222_222,
    });
    let pairing = agent_calls(&effects)[0].pairing;
    assert_eq!(h.model.next_deadline(), Some(h.now + PAIRING_TIMEOUT));

    assert!(h.advance(PAIRING_TIMEOUT.as_secs() - 1).is_empty());
    let effects = h.advance(1);
    assert_eq!(sent(&effects), [Command::CancelPairDevice(phone())]);
    assert_eq!(agent_calls(&effects)[0].prompt, Prompt::Cancel);
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::Timeout))]
    );
    assert!(!h.model.devices()[0].pairing);

    // An answer that arrives after the end does nothing, and neither does the kernel's late reply.
    assert!(h
        .answer(AgentAnswer {
            pairing,
            reply: AgentReply::Confirm(true)
        })
        .is_empty());
    assert!(completions(&h.reply(
        Opcode::PAIR_DEVICE,
        Status::Cancelled,
        device_reply(phone())
    ))
    .is_empty());
}

#[test]
fn only_one_pairing_runs_at_a_time() {
    let mut h = discovered();
    h.discover(keyboard());
    start_pairing(&mut h);
    let (ticket, effects) = h.request(Request::Pair(DeviceRef {
        address: keyboard().address,
        kind: None,
    }));
    assert_eq!(completions(&effects), [(ticket, Err(RequestError::Busy))]);

    // A request from another device meanwhile is refused to the kernel.
    let effects = h.packet(Event::UserConfirmationRequest {
        device: keyboard(),
        just_works: true,
        value: 0,
    });
    assert_eq!(
        sent(&effects),
        [Command::UserConfirmationReply {
            device: keyboard(),
            accept: false
        }]
    );
    assert!(agent_calls(&effects).is_empty());
}

#[test]
fn pairing_with_a_paired_device_or_without_power_is_refused_up_front() {
    let mut h = discovered();
    h.packet(Event::NewLinkKey {
        store: true,
        key: link_key(),
    });
    let (ticket, effects) = h.request(Request::Pair(phone_ref()));
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::AlreadyPaired))]
    );

    let mut h = discovered();
    h.packet(Event::NewSettings(cupertino_bluetooth::Settings(OFF)));
    let (ticket, effects) = h.request(Request::Pair(phone_ref()));
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::NotPowered))]
    );
}

#[test]
fn a_pairing_the_remote_side_starts_is_confirmed_and_ends_with_its_key() {
    let mut h = Harness::powered();
    // The device was never discovered; it pairs with us.
    let effects = h.packet(Event::UserConfirmationRequest {
        device: keyboard(),
        just_works: false,
        value: 999_999,
    });
    let calls = agent_calls(&effects);
    assert_eq!(
        calls[0].prompt,
        Prompt::Confirm {
            passkey: 999_999,
            just_works: false
        }
    );
    assert!(h.model.devices()[0].pairing);

    h.answer(AgentAnswer {
        pairing: calls[0].pairing,
        reply: AgentReply::Confirm(true),
    });
    let key = LinkKey {
        device: keyboard(),
        ..link_key()
    };
    let effects = h.packet(Event::NewLinkKey { store: true, key });
    assert!(effects.iter().any(|e| matches!(e, Effect::SaveKeys { .. })));
    let info = &h.model.devices()[0];
    assert_eq!(
        (info.paired, info.bonded, info.pairing),
        (true, true, false)
    );
}

#[test]
fn a_pairing_the_remote_side_started_and_lost_is_logged() {
    let mut h = Harness::powered();
    let effects = h.packet(Event::UserConfirmationRequest {
        device: keyboard(),
        just_works: true,
        value: 0,
    });
    let effects = h.answer(AgentAnswer {
        pairing: agent_calls(&effects)[0].pairing,
        reply: AgentReply::Confirm(false),
    });
    assert_eq!(sent(&effects).len(), 1);
    let effects = h.packet(Event::AuthenticationFailed {
        device: keyboard(),
        status: Status::AuthenticationFailed,
    });
    assert!(effects.contains(&Effect::Log(
        "pairing with 11:22:33:44:55:02 failed: the pairing was refused".to_owned()
    )));
    assert!(!h.model.devices()[0].pairing);
}

#[test]
fn a_device_that_wants_a_legacy_pin_is_turned_away() {
    let mut h = discovered();
    let ticket = start_pairing(&mut h);
    let effects = h.packet(Event::PinCodeRequest {
        device: phone(),
        secure: false,
    });
    assert_eq!(sent(&effects), [Command::PinCodeNegativeReply(phone())]);
    let effects = h.reply(
        Opcode::PAIR_DEVICE,
        Status::AuthenticationFailed,
        device_reply(phone()),
    );
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::PinUnsupported))]
    );
}

#[test]
fn powering_off_in_the_middle_of_a_pairing_cancels_it() {
    let mut h = discovered();
    let ticket = start_pairing(&mut h);
    let effects = h.packet(Event::UserConfirmationRequest {
        device: phone(),
        just_works: false,
        value: 5,
    });
    assert_eq!(agent_calls(&effects).len(), 1);
    let effects = h.packet(Event::NewSettings(cupertino_bluetooth::Settings(OFF)));
    assert_eq!(agent_calls(&effects)[0].prompt, Prompt::Cancel);
    assert_eq!(
        completions(&effects),
        [(ticket, Err(RequestError::Cancelled))]
    );
}
