//! The lock screen's side of PAM's conversation (`cupertino_login::auth::PasswordConversation`), the boundary this
//! crate's tests reach: the password answers the hidden prompt alone, a visible prompt is refused, and what the modules
//! say goes to the screen. PAM itself is the machine's; no test module of PAM's stands in for it.
#![cfg(feature = "app")]

use cupertino_login::auth::PasswordConversation;
use cupertino_login::model::password;
use pam_client2::{ConversationHandler, ErrorCode};
use std::cell::RefCell;

#[test]
fn the_password_answers_the_hidden_prompt_alone() {
    let said = RefCell::new(Vec::new());
    let on_notice = |text: String| said.borrow_mut().push(text);
    let mut conversation = PasswordConversation::new(password("s3cret"), &on_notice);
    assert_eq!(
        conversation
            .prompt_echo_off(c"Password: ")
            .unwrap()
            .as_bytes(),
        b"s3cret"
    );
    assert_eq!(
        conversation.prompt_echo_on(c"login: ").unwrap_err(),
        ErrorCode::CONV_ERR,
        "the lock screen asks nothing it would show"
    );
    conversation.text_info(c"Your password expires in 3 days.");
    conversation.error_msg(c"Account locked for 10 minutes.");
    assert_eq!(
        *said.borrow(),
        [
            "Your password expires in 3 days.",
            "Account locked for 10 minutes."
        ]
    );
}
