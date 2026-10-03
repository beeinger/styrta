use std::time::Duration;

use chrono::{DateTime, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use uuid::Uuid;

pub const ACCESS_TTL: Duration = Duration::from_secs(60 * 60);
pub const REFRESH_TTL: Duration = Duration::from_secs(60 * 60 * 24 * 30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Access,
    Refresh,
}

impl TokenKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Access => "access",
            Self::Refresh => "refresh",
        }
    }

    fn parse(value: &str) -> Result<Self, Error> {
        match value {
            "access" => Ok(Self::Access),
            "refresh" => Ok(Self::Refresh),
            _ => Err(Error::Invalid("typ")),
        }
    }
}

impl std::fmt::Display for TokenKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issued {
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claims {
    pub sub: Uuid,
    pub exp: DateTime<Utc>,
    pub iat: DateTime<Utc>,
    pub typ: TokenKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Expired,
    BadSignature,
    WrongType {
        expected: TokenKind,
        actual: TokenKind,
    },
    Invalid(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Expired => f.write_str("token expired"),
            Self::BadSignature => f.write_str("bad signature"),
            Self::WrongType { expected, actual } => {
                write!(f, "token type {actual} is not {expected}")
            }
            Self::Invalid(what) => write!(f, "invalid token: {what}"),
        }
    }
}

impl std::error::Error for Error {}

pub fn issue_access(secret: &str, user_id: Uuid) -> Result<Issued, Error> {
    issue_at(secret, user_id, TokenKind::Access, Utc::now())
}

pub fn issue_refresh(secret: &str, user_id: Uuid) -> Result<Issued, Error> {
    issue_at(secret, user_id, TokenKind::Refresh, Utc::now())
}

pub fn verify(secret: &str, token: &str, expected: TokenKind) -> Result<Claims, Error> {
    if secret.is_empty() {
        return Err(Error::Invalid("secret"));
    }
    let mut validation = Validation::new(Algorithm::HS256);
    validation.leeway = 0;
    validation.validate_exp = true;
    validation.validate_nbf = false;
    let data = decode::<JwtClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(map_jwt)?;
    let kind = TokenKind::parse(&data.claims.typ)?;
    if kind != expected {
        return Err(Error::WrongType {
            expected,
            actual: kind,
        });
    }
    let sub = Uuid::parse_str(&data.claims.sub).map_err(|_| Error::Invalid("sub"))?;
    Ok(Claims {
        sub,
        exp: from_unix(data.claims.exp)?,
        iat: from_unix(data.claims.iat)?,
        typ: kind,
    })
}

fn issue_at(
    secret: &str,
    user_id: Uuid,
    kind: TokenKind,
    now: DateTime<Utc>,
) -> Result<Issued, Error> {
    if secret.is_empty() {
        return Err(Error::Invalid("secret"));
    }
    let ttl_secs: i64 = match kind {
        TokenKind::Access => 60 * 60,
        TokenKind::Refresh => 60 * 60 * 24 * 30,
    };
    let iat = now.timestamp();
    let exp = iat.checked_add(ttl_secs).ok_or(Error::Invalid("exp"))?;
    let claims = JwtClaims {
        sub: user_id.to_string(),
        exp,
        iat,
        typ: kind.as_str().to_string(),
    };
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|_| Error::Invalid("token"))?;
    Ok(Issued {
        token,
        expires_at: from_unix(exp)?,
    })
}

fn from_unix(seconds: i64) -> Result<DateTime<Utc>, Error> {
    DateTime::from_timestamp(seconds, 0).ok_or(Error::Invalid("exp"))
}

fn map_jwt(err: jsonwebtoken::errors::Error) -> Error {
    use jsonwebtoken::errors::ErrorKind;
    match err.kind() {
        ErrorKind::ExpiredSignature => Error::Expired,
        ErrorKind::InvalidSignature => Error::BadSignature,
        _ => Error::Invalid("token"),
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct JwtClaims {
    sub: String,
    exp: i64,
    iat: i64,
    typ: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-jwt-secret";

    fn user() -> Uuid {
        Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap()
    }

    #[test]
    fn access_and_refresh_round_trip() {
        let access = issue_access(SECRET, user()).unwrap();
        let claims = verify(SECRET, &access.token, TokenKind::Access).unwrap();
        assert_eq!(claims.sub, user());
        assert_eq!(claims.typ, TokenKind::Access);
        assert_eq!(claims.exp.timestamp() - claims.iat.timestamp(), 3600);
        assert_eq!(access.expires_at, claims.exp);

        let refresh = issue_refresh(SECRET, user()).unwrap();
        let claims = verify(SECRET, &refresh.token, TokenKind::Refresh).unwrap();
        assert_eq!(claims.typ, TokenKind::Refresh);
        assert_eq!(
            claims.exp.timestamp() - claims.iat.timestamp(),
            30 * 24 * 3600
        );
    }

    #[test]
    fn wrong_type_is_rejected() {
        let refresh = issue_refresh(SECRET, user()).unwrap();
        let err = verify(SECRET, &refresh.token, TokenKind::Access).unwrap_err();
        assert_eq!(
            err,
            Error::WrongType {
                expected: TokenKind::Access,
                actual: TokenKind::Refresh,
            }
        );
    }

    #[test]
    fn expired_token_is_rejected() {
        let issued = issue_at(
            SECRET,
            user(),
            TokenKind::Access,
            Utc::now() - chrono::Duration::hours(2),
        )
        .unwrap();
        let err = verify(SECRET, &issued.token, TokenKind::Access).unwrap_err();
        assert_eq!(err, Error::Expired);
    }

    #[test]
    fn bad_signature_is_rejected() {
        let issued = issue_access(SECRET, user()).unwrap();
        let err = verify("other-jwt-secret", &issued.token, TokenKind::Access).unwrap_err();
        assert_eq!(err, Error::BadSignature);
        assert!(!err.to_string().contains(SECRET));
    }
}
