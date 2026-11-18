use std::time::Duration;

use cupertino_btd::bluetooth::{AddressType, Command, DeviceId, Status};
use cupertino_btd::pairing::{Action, AgentReply, PairError, Pairing, PairingId, Prompt};

#[test]
fn a_local_confirmation_is_answered_once_and_pair_reports_the_cause_to_the_caller() {
    let device = DeviceId {
        address: "02:00:00:00:00:04".parse().unwrap(),
        kind: AddressType::LePublic,
    };
    for (reply, accept, status, result) in [
        (AgentReply::Confirm(true), true, Status::Success, Ok(())),
        (
            AgentReply::Confirm(false),
            false,
            Status::AuthenticationFailed,
            Err(PairError::Rejected),
        ),
        (
            AgentReply::Failed("agent closed".into()),
            false,
            Status::Failed,
            Err(PairError::Agent("agent closed".into())),
        ),
    ] {
        let mut pairing = Pairing::new(PairingId(1), device, true, Duration::ZERO);
        assert_eq!(
            pairing.on_confirm_request(false, 314159),
            [Action::Prompt(Prompt::Confirm {
                passkey: 314159,
                just_works: false
            })]
        );
        assert!(pairing
            .on_agent_reply(AgentReply::Passkey(Some(7)))
            .is_empty());
        assert_eq!(
            pairing.on_agent_reply(reply.clone()),
            [Action::Send(Command::UserConfirmationReply {
                device,
                accept
            })]
        );
        assert!(pairing.on_agent_reply(reply).is_empty());
        assert_eq!(pairing.on_pair_reply(status), [Action::Finish(result)]);
    }
}
