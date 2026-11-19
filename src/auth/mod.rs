//! Login flows and the authentication state machine.
//!
//! There is exactly one identity: the librespot session, opened with the
//! public desktop client id. The flow is:
//! 1. `Restoring`: on boot, look for librespot's reusable credentials in the
//!    keyring and, if present, open the session silently (no browser).
//! 2. `SignedOut`: no (usable) stored credentials. "Log in" starts PKCE and
//!    opens accounts.spotify.com in the browser.
//! 3. `WaitingForBrowser`: the loopback listener waits for the redirect.
//! 4. `Connecting`: exchange the code and open the librespot session with the
//!    one-off access token; store the session's reusable credentials.
//! 5. `SignedIn`.
//!
//! If the silent login is rejected (credentials revoked, password changed),
//! the stored credentials are deleted and the state falls back to
//! `SignedOut` with an explanation, so the next click opens the browser.

pub mod oauth;
pub mod secrets;

use crate::error::Error;
use crate::net::model::User;

#[derive(Debug, Clone, PartialEq)]
pub enum AuthState {
    /// Reading the keyring / opening the session from stored credentials.
    Restoring,
    /// No usable session. `error` is shown under the login button.
    SignedOut {
        error: Option<String>,
    },
    /// The authorize URL was opened; waiting for the loopback redirect.
    WaitingForBrowser {
        url: String,
    },
    /// Exchanging the code / connecting the session.
    Connecting,
    /// No usable OS keyring (Secret Service): show setup guidance + retry.
    KeyringUnavailable {
        detail: String,
    },
    SignedIn {
        user: User,
    },
}

#[derive(Debug, Clone)]
pub enum AuthEvent {
    NoStoredSession,
    LoginStarted {
        url: String,
    },
    CallbackReceived,
    StoredSessionFound,
    /// Stored credentials were refused: fall back to the browser login.
    StoredSessionRejected,
    Connected(User),
    Failed(Error),
    LoggedOut,
}

/// Shown when the silent login fails and the browser login is needed again.
pub const SILENT_LOGIN_REJECTED: &str =
    "Your saved login is no longer valid. Log in again to continue.";

impl AuthState {
    pub fn initial() -> Self {
        Self::Restoring
    }

    pub fn is_signed_in(&self) -> bool {
        matches!(self, Self::SignedIn { .. })
    }

    /// Pure transition function; the app performs the side effects.
    pub fn apply(&self, event: AuthEvent) -> Self {
        use AuthEvent as E;
        match (self, event) {
            (_, E::NoStoredSession) => Self::SignedOut { error: None },
            (_, E::StoredSessionRejected) => Self::SignedOut {
                error: Some(SILENT_LOGIN_REJECTED.into()),
            },
            (_, E::LoginStarted { url }) => Self::WaitingForBrowser { url },
            (_, E::CallbackReceived | E::StoredSessionFound) => Self::Connecting,
            (_, E::Connected(user)) => Self::SignedIn { user },
            // A failure while already signed in (e.g. a transient network
            // error) keeps the session unless it was revoked.
            (Self::SignedIn { user }, E::Failed(e)) if !e.is_revoked() => {
                Self::SignedIn { user: user.clone() }
            }
            (_, E::Failed(Error::Keyring(detail))) => Self::KeyringUnavailable { detail },
            (_, E::Failed(e)) => Self::SignedOut {
                error: Some(e.user_message()),
            },
            (_, E::LoggedOut) => Self::SignedOut { error: None },
        }
    }
}

/// What a failed silent login means: revoked credentials fall back to the
/// browser login (and must be deleted); anything else (offline, Spotify down)
/// is reported but the credentials are kept for the next start.
pub fn silent_login_failure(e: &Error) -> (AuthEvent, bool) {
    if e.is_revoked() {
        (AuthEvent::StoredSessionRejected, true)
    } else {
        (AuthEvent::Failed(e.clone()), false)
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
    fn browser_login_happy_path() {
        let s = AuthState::initial();
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
    fn silent_login_success_skips_the_browser() {
        let s = AuthState::initial()
            .apply(AuthEvent::StoredSessionFound)
            .apply(AuthEvent::Connected(user()));
        assert!(s.is_signed_in());
    }

    #[test]
    fn silent_login_rejected_falls_back_to_browser_login() {
        let (ev, delete) = silent_login_failure(&Error::revoked("bad credentials"));
        assert!(delete, "revoked credentials are deleted");
        let s = AuthState::Connecting.apply(ev);
        assert_eq!(
            s,
            AuthState::SignedOut {
                error: Some(SILENT_LOGIN_REJECTED.into())
            }
        );
        // ...and the next click starts the browser flow.
        let s = s.apply(AuthEvent::LoginStarted { url: "u".into() });
        assert!(matches!(s, AuthState::WaitingForBrowser { .. }));
    }

    #[test]
    fn silent_login_offline_keeps_credentials() {
        let (ev, delete) = silent_login_failure(&Error::Net("offline".into()));
        assert!(!delete);
        assert!(matches!(
            AuthState::Connecting.apply(ev),
            AuthState::SignedOut { error: Some(_) }
        ));
    }

    #[test]
    fn keyring_failures_show_guidance() {
        let s = AuthState::Connecting.apply(AuthEvent::Failed(Error::Keyring("no wallet".into())));
        assert_eq!(
            s,
            AuthState::KeyringUnavailable {
                detail: "no wallet".into()
            }
        );
    }

    #[test]
    fn failures_while_signed_in() {
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
}
