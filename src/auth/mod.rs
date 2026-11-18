//! Login flows and the authentication state machine.
//!
//! The flow is:
//! 1. `Restoring`: on boot, look for tokens + librespot credentials in the keyring.
//! 2. `SignedOut`: show the login screen. "Log in" starts PKCE and opens a browser.
//! 3. `WaitingForBrowser`: loopback listener waits for the redirect.
//! 4. `Connecting`: exchange the code, fetch the profile, open the librespot session.
//! 5. `SignedIn`.

pub mod oauth;
pub mod secrets;

use crate::error::Error;
use crate::net::model::User;

#[derive(Debug, Clone, PartialEq)]
pub enum AuthState {
    /// Reading the keyring at startup.
    Restoring,
    /// No usable session. `error` is shown under the login button.
    SignedOut {
        error: Option<String>,
    },
    /// No client id configured: the login screen explains how to set one.
    MissingClientId,
    /// The authorize URL was opened; waiting for the loopback redirect.
    WaitingForBrowser {
        url: String,
    },
    /// Exchanging the code / connecting the audio session.
    Connecting,
    SignedIn {
        user: User,
    },
}

#[derive(Debug, Clone)]
pub enum AuthEvent {
    NoStoredSession,
    LoginStarted { url: String },
    CallbackReceived,
    StoredSessionFound,
    Connected(User),
    Failed(Error),
    LoggedOut,
}

impl AuthState {
    pub fn initial(has_client_id: bool) -> Self {
        if has_client_id {
            Self::Restoring
        } else {
            Self::MissingClientId
        }
    }

    pub fn is_signed_in(&self) -> bool {
        matches!(self, Self::SignedIn { .. })
    }

    /// Pure transition function; the app performs the side effects.
    pub fn apply(&self, event: AuthEvent) -> Self {
        use AuthEvent as E;
        match (self, event) {
            (Self::MissingClientId, E::Connected(user)) => Self::SignedIn { user },
            (Self::MissingClientId, _) => Self::MissingClientId,
            (_, E::NoStoredSession) => Self::SignedOut { error: None },
            (_, E::LoginStarted { url }) => Self::WaitingForBrowser { url },
            (_, E::CallbackReceived | E::StoredSessionFound) => Self::Connecting,
            (_, E::Connected(user)) => Self::SignedIn { user },
            // A failure while already signed in (e.g. a refresh that failed
            // transiently) keeps the session unless it was revoked.
            (Self::SignedIn { user }, E::Failed(e)) if !e.is_revoked() => {
                Self::SignedIn { user: user.clone() }
            }
            (_, E::Failed(e)) => Self::SignedOut {
                error: Some(e.user_message()),
            },
            (_, E::LoggedOut) => Self::SignedOut { error: None },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> User {
        User {
            id: "u1".into(),
            display_name: "Test".into(),
            image: None,
            product: Some("premium".into()),
        }
    }

    #[test]
    fn happy_path() {
        let s = AuthState::initial(true);
        assert_eq!(s, AuthState::Restoring);
        let s = s.apply(AuthEvent::NoStoredSession);
        assert_eq!(s, AuthState::SignedOut { error: None });
        let s = s.apply(AuthEvent::LoginStarted { url: "u".into() });
        assert!(matches!(s, AuthState::WaitingForBrowser { .. }));
        let s = s.apply(AuthEvent::CallbackReceived);
        assert_eq!(s, AuthState::Connecting);
        let s = s.apply(AuthEvent::Connected(user()));
        assert!(s.is_signed_in());
        let s = s.apply(AuthEvent::LoggedOut);
        assert_eq!(s, AuthState::SignedOut { error: None });
    }

    #[test]
    fn failures() {
        let s = AuthState::Connecting.apply(AuthEvent::Failed(Error::auth("nope")));
        assert_eq!(
            s,
            AuthState::SignedOut {
                error: Some("nope".into())
            }
        );
        let signed = AuthState::SignedIn { user: user() };
        assert!(
            signed
                .apply(AuthEvent::Failed(Error::Net("x".into())))
                .is_signed_in()
        );
        assert!(
            !signed
                .apply(AuthEvent::Failed(Error::revoked("gone")))
                .is_signed_in()
        );
    }

    #[test]
    fn missing_client_id_is_sticky() {
        let s = AuthState::initial(false);
        assert_eq!(s, AuthState::MissingClientId);
        assert_eq!(
            s.apply(AuthEvent::NoStoredSession),
            AuthState::MissingClientId
        );
    }
}
