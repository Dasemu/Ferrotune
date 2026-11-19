//! Access tokens for spclient and pathfinder, issued by the librespot session.
//!
//! There is no separate OAuth app: tokens come from the authenticated
//! session (login5 with the session's stored credentials, exactly what the
//! desktop client and librespot's own spclient use). `TokenCache` adds
//! expiry handling and single-flight refresh on top of any [`TokenSource`].

use std::time::{Duration, Instant};

use futures::future::BoxFuture;

use crate::auth::secrets::Secret;
use crate::error::{Error, Result};

/// Refresh this long before the token expires.
const MARGIN: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq)]
pub struct AccessToken {
    pub secret: Secret,
    pub expires_at: Instant,
}

impl AccessToken {
    pub fn is_fresh(&self, now: Instant) -> bool {
        now + MARGIN < self.expires_at
    }
}

/// Something that can mint a new access token.
pub trait TokenSource: Send + Sync {
    fn fetch(&self) -> BoxFuture<'_, Result<AccessToken>>;
}

/// Caches the current token; concurrent callers share one refresh.
pub struct TokenCache {
    current: tokio::sync::Mutex<Option<AccessToken>>,
}

impl Default for TokenCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenCache {
    pub fn new() -> Self {
        Self {
            current: tokio::sync::Mutex::new(None),
        }
    }

    /// A fresh token, fetching one if needed. The lock is held while
    /// fetching, so N parallel callers trigger exactly one fetch.
    pub async fn get(&self, source: &dyn TokenSource) -> Result<Secret> {
        let mut cur = self.current.lock().await;
        if let Some(t) = cur.as_ref().filter(|t| t.is_fresh(Instant::now())) {
            return Ok(t.secret.clone());
        }
        let t = source.fetch().await?;
        let secret = t.secret.clone();
        *cur = Some(t);
        Ok(secret)
    }

    /// Called after a 401 for a request made with `stale`. If another request
    /// already replaced the token, that one is returned without a new fetch.
    pub async fn refresh_after(&self, source: &dyn TokenSource, stale: &Secret) -> Result<Secret> {
        let mut cur = self.current.lock().await;
        if let Some(t) = cur.as_ref()
            && t.secret != *stale
            && t.is_fresh(Instant::now())
        {
            return Ok(t.secret.clone());
        }
        *cur = None;
        let t = source.fetch().await?;
        if t.secret == *stale {
            // The session handed back the token Spotify just refused.
            return Err(Error::auth(
                "Spotify refused the session token. Reconnecting may help.",
            ));
        }
        let secret = t.secret.clone();
        *cur = Some(t);
        Ok(secret)
    }

    /// Forget the token (session replaced after a reconnect).
    pub async fn clear(&self) {
        *self.current.lock().await = None;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    struct Counting {
        calls: AtomicU32,
        ttl: Duration,
    }

    impl TokenSource for Counting {
        fn fetch(&self) -> BoxFuture<'_, Result<AccessToken>> {
            Box::pin(async move {
                let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(AccessToken {
                    secret: Secret::new(format!("t{n}")),
                    expires_at: Instant::now() + self.ttl,
                })
            })
        }
    }

    fn source(ttl: Duration) -> Arc<Counting> {
        Arc::new(Counting {
            calls: AtomicU32::new(0),
            ttl,
        })
    }

    #[tokio::test]
    async fn parallel_callers_share_one_fetch() {
        let src = source(Duration::from_secs(3600));
        let cache = Arc::new(TokenCache::new());
        let mut joins = Vec::new();
        for _ in 0..8 {
            let (c, s) = (cache.clone(), src.clone());
            joins.push(tokio::spawn(async move { c.get(s.as_ref()).await }));
        }
        for j in joins {
            assert_eq!(j.await.unwrap().unwrap().expose(), "t1");
        }
        assert_eq!(src.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn expiring_tokens_are_refreshed() {
        let src = source(Duration::from_secs(30)); // inside the 60 s margin
        let cache = TokenCache::new();
        assert_eq!(cache.get(src.as_ref()).await.unwrap().expose(), "t1");
        assert_eq!(cache.get(src.as_ref()).await.unwrap().expose(), "t2");
    }

    #[tokio::test]
    async fn refresh_after_401_is_single_flight() {
        let src = source(Duration::from_secs(3600));
        let cache = Arc::new(TokenCache::new());
        let stale = cache.get(src.as_ref()).await.unwrap();
        let mut joins = Vec::new();
        for _ in 0..5 {
            let (c, s, st) = (cache.clone(), src.clone(), stale.clone());
            joins.push(tokio::spawn(async move {
                c.refresh_after(s.as_ref(), &st).await
            }));
        }
        for j in joins {
            assert_eq!(j.await.unwrap().unwrap().expose(), "t2");
        }
        assert_eq!(
            src.calls.load(Ordering::SeqCst),
            2,
            "one initial + one refresh"
        );
    }

    struct Same;
    impl TokenSource for Same {
        fn fetch(&self) -> BoxFuture<'_, Result<AccessToken>> {
            Box::pin(async {
                Ok(AccessToken {
                    secret: Secret::new("same"),
                    expires_at: Instant::now() + Duration::from_secs(3600),
                })
            })
        }
    }

    #[tokio::test]
    async fn refused_token_handed_back_is_an_error() {
        let cache = TokenCache::new();
        let stale = cache.get(&Same).await.unwrap();
        assert!(cache.refresh_after(&Same, &stale).await.is_err());
    }
}
