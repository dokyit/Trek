//! The two WebAuthn ceremonies as Trek's Browser runs them: a page's `navigator.credentials
//! .create()` or `.get()` request is read and checked against the page's own origin ([`parse`]),
//! the Browser asks the user, and `finish` verifies them and answers with what the page's
//! `PublicKeyCredential` is made of.
//!
//! The page's script supplies the request and nothing else: the origin comes from the web view,
//! and everything here refuses rather than guesses. A request arrives as the JSON form of
//! WebAuthn's own option dictionaries (`PublicKeyCredentialCreationOptionsJSON` and
//! `…RequestOptionsJSON`, binary values as base64url) and the answer leaves as
//! `RegistrationResponseJSON` or `AuthenticationResponseJSON`.

use crate::keys::{Authenticator, KeyError};
use crate::rp::{self, Origin};
use crate::store::{Passkey, Store};
use crate::wire::{self, Ceremony, b64, flags, unb64};
use serde_json::{Value, json};

/// Why a ceremony ended without a credential: the exception the page's promise rejects with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// A `DOMException` name, or `TypeError`.
    pub name: &'static str,
    pub message: String,
}

impl Error {
    fn new(name: &'static str, message: impl Into<String>) -> Error {
        Error {
            name,
            message: message.into(),
        }
    }

    /// The user said no, nothing fits, or the page may not ask: what WebAuthn answers whenever
    /// saying more would tell a page something about the user.
    pub fn not_allowed(message: impl Into<String>) -> Error {
        Error::new("NotAllowedError", message)
    }

    pub fn security(message: impl Into<String>) -> Error {
        Error::new("SecurityError", message)
    }

    pub fn invalid_state(message: impl Into<String>) -> Error {
        Error::new("InvalidStateError", message)
    }

    fn not_supported(message: impl Into<String>) -> Error {
        Error::new("NotSupportedError", message)
    }

    fn type_error(message: impl Into<String>) -> Error {
        Error::new("TypeError", message)
    }

    /// As the bridge sends it to the page.
    pub fn to_json(&self) -> Value {
        json!({ "error": { "name": self.name, "message": self.message } })
    }
}

impl From<KeyError> for Error {
    fn from(e: KeyError) -> Error {
        match e {
            KeyError::Cancelled => Error::not_allowed("The request was cancelled."),
            KeyError::Failed(why) => {
                tracing::warn!("passkeys: {why}");
                Error::not_allowed("The passkey couldn't be used.")
            }
        }
    }
}

/// What a page asked for, read and checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Create(Create),
    Get(Get),
}

/// A request for a new passkey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Create {
    /// The site the passkey will be for, checked against the page's origin.
    pub rp_id: String,
    pub rp_name: String,
    pub user_handle: Vec<u8>,
    pub user_name: String,
    pub user_display_name: String,
    challenge: Vec<u8>,
    origin: String,
    exclude: Vec<Vec<u8>>,
    cred_props: bool,
}

/// A request to sign in with a passkey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Get {
    /// The site signed in to, checked against the page's origin.
    pub rp_id: String,
    challenge: Vec<u8>,
    origin: String,
    /// The credentials the site will take, when it named any (`Some`, which may hold none of a
    /// kind known here); `None` for "whichever passkey the user has".
    allow: Option<Vec<Vec<u8>>>,
    /// The page asked without wanting a dialog (`mediation: "conditional"`, for autofill).
    pub conditional: bool,
    appid: bool,
}

/// The largest request body read: a page sends a challenge and a few ids, never megabytes.
pub const MAX_REQUEST: usize = 256 * 1024;

/// Read a page's request (`{"kind": "create" | "get", "publicKey": {…}, "mediation": …}`) from
/// the frame whose origin is `origin`.
pub fn parse(origin: &Origin, body: &str) -> Result<Request, Error> {
    if body.len() > MAX_REQUEST {
        return Err(Error::type_error("The request is too large."));
    }
    let body: Value =
        serde_json::from_str(body).map_err(|_| Error::type_error("The request can't be read."))?;
    // The origin first: a page that may not use passkeys learns nothing else.
    origin.effective_domain().map_err(Error::security)?;
    let options = body
        .get("publicKey")
        .filter(|v| v.is_object())
        .ok_or_else(|| Error::type_error("publicKey is required."))?;
    let mediation = body.get("mediation").and_then(Value::as_str);
    let conditional = mediation == Some("conditional");
    // A request that must not put anything in front of the user can't be answered here: every
    // use of a passkey is one the user was asked about. (The same answer whatever Trek keeps.)
    if mediation == Some("silent") {
        return Err(Error::not_allowed(
            "Passkeys are only used when you're asked, not silently.",
        ));
    }
    match body.get("kind").and_then(Value::as_str) {
        // A passkey made without a dialog (a site upgrading a password sign-in on the quiet) is
        // not something Trek does: every passkey here is one the user was asked about.
        Some("create") if conditional => Err(Error::not_allowed(
            "Passkeys are only created when you're asked, not automatically.",
        )),
        Some("create") => parse_create(origin, options).map(Request::Create),
        Some("get") => parse_get(origin, options, conditional).map(Request::Get),
        _ => Err(Error::type_error("The request can't be read.")),
    }
}

fn required_text(of: &Value, key: &str, what: &str) -> Result<String, Error> {
    of.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::type_error(format!("{what} is required.")))
}

fn required_bytes(of: &Value, key: &str, what: &str) -> Result<Vec<u8>, Error> {
    let text = required_text(of, key, what)?;
    unb64(&text).ok_or_else(|| Error::type_error(format!("{what} isn't base64url.")))
}

/// The credential ids a descriptor list names (`allowCredentials`, `excludeCredentials`):
/// those of type `public-key`, as WebAuthn says to ignore the rest.
fn descriptor_ids(list: Option<&Value>, what: &str) -> Result<Vec<Vec<u8>>, Error> {
    let Some(list) = list.filter(|v| !v.is_null()) else {
        return Ok(Vec::new());
    };
    let list = list
        .as_array()
        .ok_or_else(|| Error::type_error(format!("{what} must be a list.")))?;
    let mut ids = Vec::new();
    for item in list {
        let id = required_bytes(item, "id", &format!("{what}.id"))?;
        let kind = required_text(item, "type", &format!("{what}.type"))?;
        if kind == "public-key" {
            ids.push(id);
        }
    }
    Ok(ids)
}

fn claimed_rp(origin: &Origin, claimed: Option<&Value>) -> Result<String, Error> {
    let claimed = match claimed {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.as_str()),
        Some(_) => return Err(Error::type_error("The relying party id must be text.")),
    };
    rp::relying_party(origin, claimed).map_err(Error::security)
}

fn parse_create(origin: &Origin, options: &Value) -> Result<Create, Error> {
    let rp = options
        .get("rp")
        .filter(|v| v.is_object())
        .ok_or_else(|| Error::type_error("rp is required."))?;
    let user = options
        .get("user")
        .filter(|v| v.is_object())
        .ok_or_else(|| Error::type_error("user is required."))?;
    let rp_name = required_text(rp, "name", "rp.name")?;
    let user_handle = required_bytes(user, "id", "user.id")?;
    let user_name = required_text(user, "name", "user.name")?;
    let user_display_name = required_text(user, "displayName", "user.displayName")?;
    let challenge = required_bytes(options, "challenge", "challenge")?;
    if user_handle.is_empty() || user_handle.len() > 64 {
        return Err(Error::type_error("user.id must be 1 to 64 bytes."));
    }
    let rp_id = claimed_rp(origin, rp.get("id"))?;

    // The algorithm: the site lists what it takes, and Trek's keys are ES256 alone. An empty
    // list means the specification's defaults, which start with ES256.
    let params = options
        .get("pubKeyCredParams")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::type_error("pubKeyCredParams is required."))?;
    let es256 = params.is_empty()
        || params.iter().any(|p| {
            p.get("type").and_then(Value::as_str) == Some("public-key")
                && p.get("alg").and_then(Value::as_i64) == Some(wire::ES256)
        });
    if !es256 {
        return Err(Error::not_supported(
            "None of the algorithms the site takes is ES256, the one Trek's passkeys use.",
        ));
    }
    // A site that wants a key on a separate device (a security key) can't have this one.
    let attachment = options
        .pointer("/authenticatorSelection/authenticatorAttachment")
        .and_then(Value::as_str);
    if attachment == Some("cross-platform") {
        return Err(Error::not_allowed(
            "The site asked for a security key. Trek's Browser keeps passkeys on this computer only.",
        ));
    }
    Ok(Create {
        rp_id,
        rp_name,
        user_handle,
        user_name,
        user_display_name,
        challenge,
        origin: origin.serialize(),
        exclude: descriptor_ids(options.get("excludeCredentials"), "excludeCredentials")?,
        cred_props: options
            .pointer("/extensions/credProps")
            .and_then(Value::as_bool)
            == Some(true),
    })
}

fn parse_get(origin: &Origin, options: &Value, conditional: bool) -> Result<Get, Error> {
    let challenge = required_bytes(options, "challenge", "challenge")?;
    let rp_id = claimed_rp(origin, options.get("rpId"))?;
    Ok(Get {
        rp_id,
        challenge,
        origin: origin.serialize(),
        // A list was given when it has anything in it, of whatever type: one whose every entry
        // is of a type unknown here names no passkey of Trek's, which is not "any passkey".
        allow: options
            .get("allowCredentials")
            .and_then(Value::as_array)
            .is_some_and(|list| !list.is_empty())
            .then(|| descriptor_ids(options.get("allowCredentials"), "allowCredentials"))
            .transpose()?,
        conditional,
        appid: options
            .pointer("/extensions/appid")
            .is_some_and(|v| !v.is_null()),
    })
}

fn storage(e: String) -> Error {
    tracing::warn!("passkeys: {e}");
    Error::not_allowed("Trek's passkeys can't be read.")
}

impl Create {
    /// Whether the site already has one of Trek's passkeys for this account (it listed it in
    /// `excludeCredentials`): the user is told, and the page gets `InvalidStateError`.
    pub fn already(&self, store: &Store) -> Result<bool, Error> {
        let here = store.for_site(&self.rp_id).map_err(storage)?;
        Ok(here
            .iter()
            .any(|p| self.exclude.iter().any(|id| b64(id) == p.id)))
    }

    /// Whether Trek keeps a passkey for this account on the site already, which the new one
    /// will take the place of.
    pub fn replaces(&self, store: &Store) -> Result<bool, Error> {
        store
            .for_account(&self.rp_id, &b64(&self.user_handle))
            .map(|kept| !kept.is_empty())
            .map_err(storage)
    }

    /// Verify the user, make the key and keep the passkey: the page's answer, and the passkey,
    /// which the caller settles once it knows whether the page got the answer (`settle`).
    /// Waits on the user (never on the main thread).
    pub fn finish(
        &self,
        auth: &dyn Authenticator,
        store: &Store,
        now: u64,
    ) -> Result<(Value, Passkey), Error> {
        if self.already(store)? {
            return Err(Error::invalid_state(
                "A passkey for this account is already kept here.",
            ));
        }
        let reason = format!(
            "create a passkey for {} on {}",
            account_words(&self.user_name, &self.user_display_name),
            self.rp_id
        );
        let key = auth.create(&reason)?;
        let mut id = [0u8; 32];
        if let Err(e) = getrandom::fill(&mut id) {
            auth.forget(&key.handle);
            return Err(KeyError::Failed(format!("no random numbers: {e}")).into());
        }
        let auth_data =
            wire::authenticator_data(&self.rp_id, flags::UP | flags::UV, Some((&id, &key.point)));
        let client_data = wire::client_data_json(Ceremony::Create, &self.challenge, &self.origin);
        let passkey = Passkey {
            id: b64(&id),
            rp_id: self.rp_id.clone(),
            rp_name: self.rp_name.clone(),
            user_handle: b64(&self.user_handle),
            user_name: self.user_name.clone(),
            user_display_name: self.user_display_name.clone(),
            public_key: b64(&key.point),
            key: key.handle.clone(),
            created: now,
            last_used: None,
            extra: Default::default(),
        };
        // Kept before the page hears of it: a passkey the site has and Trek lost would be a
        // sign-in that can never work. One already kept for the account stays for now: it goes
        // once the page has this one (`settle`), not before.
        if let Err(e) = store.add(passkey.clone()) {
            auth.forget(&key.handle);
            return Err(storage(e));
        }
        let mut extensions = serde_json::Map::new();
        if self.cred_props {
            // Every passkey here is discoverable: the site can ask for it without naming it.
            extensions.insert("credProps".into(), json!({ "rk": true }));
        }
        let answer = json!({
            "id": passkey.id,
            "rawId": passkey.id,
            "type": "public-key",
            "authenticatorAttachment": "platform",
            "response": {
                "clientDataJSON": b64(&client_data),
                "attestationObject": b64(&wire::attestation_object(&auth_data)),
                "authenticatorData": b64(&auth_data),
                "transports": ["internal"],
                "publicKey": b64(&wire::spki(&key.point)),
                "publicKeyAlgorithm": wire::ES256,
            },
            "clientExtensionResults": extensions,
        });
        Ok((answer, passkey))
    }
}

/// A new passkey's answer reached its page (`delivered`), or the page was gone by then (it
/// navigated, closed, or took its request back while Touch ID was up). Delivered: the passkeys
/// Trek kept for the same account before go, as the site now has this one (one per account per
/// site, WebAuthn's rule for discoverable credentials). Not delivered: the site never saw the
/// new one, so it is the one that goes, and what was kept stays usable.
pub fn settle(auth: &dyn Authenticator, store: &Store, passkey: &Passkey, delivered: bool) {
    let gone = if delivered {
        store.replace_older(passkey)
    } else {
        store.remove(&passkey.id).map(|p| p.into_iter().collect())
    };
    match gone {
        Ok(gone) => {
            for old in gone {
                auth.forget(&old.key);
            }
        }
        Err(e) => tracing::warn!("passkeys: {e}"),
    }
}

impl Get {
    /// The passkeys that could answer: this site's, and when the site named the credentials it
    /// takes, only those. A passkey of another site is never one, whatever the page lists.
    pub fn candidates(&self, store: &Store) -> Result<Vec<Passkey>, Error> {
        let here = store.for_site(&self.rp_id).map_err(storage)?;
        Ok(here
            .into_iter()
            .filter(|p| match &self.allow {
                Some(allow) => allow.iter().any(|id| b64(id) == p.id),
                None => true,
            })
            .collect())
    }

    /// Verify the user and sign in with `passkey` (one of `candidates`): the page's answer.
    /// Waits on the user (never on the main thread).
    pub fn finish(
        &self,
        passkey: &Passkey,
        auth: &dyn Authenticator,
        store: &Store,
        now: u64,
    ) -> Result<Value, Error> {
        // Whatever the caller chose, only a passkey this request could have been answered with
        // is signed with, read again from the list.
        let passkey = self
            .candidates(store)?
            .into_iter()
            .find(|p| p.id == passkey.id && p.rp_id == self.rp_id)
            .ok_or_else(|| Error::not_allowed("That passkey isn't one this site can use."))?;
        let auth_data = wire::authenticator_data(&self.rp_id, flags::UP | flags::UV, None);
        let client_data = wire::client_data_json(Ceremony::Get, &self.challenge, &self.origin);
        let mut message = auth_data.clone();
        message.extend_from_slice(&wire::sha256(&client_data));
        let reason = format!("sign in to {} as {}", self.rp_id, passkey.account());
        let signature = auth.sign(&passkey.key, &message, &reason)?;
        if let Err(e) = store.used(&passkey.id, now) {
            tracing::warn!("passkeys: couldn't note the use: {e}");
        }
        let mut extensions = serde_json::Map::new();
        if self.appid {
            // The old U2F id wasn't what was signed for.
            extensions.insert("appid".into(), json!(false));
        }
        Ok(json!({
            "id": passkey.id,
            "rawId": passkey.id,
            "type": "public-key",
            "authenticatorAttachment": "platform",
            "response": {
                "clientDataJSON": b64(&client_data),
                "authenticatorData": b64(&auth_data),
                "signature": b64(&signature),
                "userHandle": passkey.user_handle,
            },
            "clientExtensionResults": extensions,
        }))
    }
}

/// An account in a sentence: its user name, else its display name, as one short line
/// (`store::named`).
pub fn account_words(user_name: &str, display_name: &str) -> String {
    crate::store::named(user_name, display_name).unwrap_or_else(|| "your account".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbor;
    use crate::keys::{StandIn, set_stand_in, verifies};
    use crate::store::tests::temp_store;
    use std::sync::Mutex;

    /// The stand-in's switch is the process's: tests that flip it take turns.
    static SWITCH: Mutex<()> = Mutex::new(());

    fn isolated() -> std::sync::MutexGuard<'static, ()> {
        trek_core::paths::isolate(std::env::temp_dir().join("trek-passkeys-tests"));
        let guard = SWITCH.lock().unwrap_or_else(|e| e.into_inner());
        set_stand_in(true);
        guard
    }

    fn origin(url: &str) -> Origin {
        Origin::parse(url).unwrap()
    }

    fn create_body(rp_id: Option<&str>, user: &[u8], name: &str) -> String {
        let mut rp = json!({ "name": "Example" });
        if let Some(id) = rp_id {
            rp["id"] = json!(id);
        }
        json!({
            "kind": "create",
            "publicKey": {
                "rp": rp,
                "user": { "id": b64(user), "name": name, "displayName": "Ada" },
                "challenge": b64(b"register-challenge"),
                "pubKeyCredParams": [
                    { "type": "public-key", "alg": -8 },
                    { "type": "public-key", "alg": -7 },
                    { "type": "public-key", "alg": -257 },
                ],
                "authenticatorSelection": { "residentKey": "required", "userVerification": "required" },
                "attestation": "none",
                "extensions": { "credProps": true },
            }
        })
        .to_string()
    }

    fn get_body(rp_id: Option<&str>, allow: &[&str]) -> String {
        let mut options =
            json!({ "challenge": b64(b"sign-in-challenge"), "userVerification": "preferred" });
        if let Some(id) = rp_id {
            options["rpId"] = json!(id);
        }
        if !allow.is_empty() {
            options["allowCredentials"] = allow
                .iter()
                .map(|id| json!({ "type": "public-key", "id": id }))
                .collect();
        }
        json!({ "kind": "get", "publicKey": options }).to_string()
    }

    fn create(o: &str, body: &str) -> Create {
        match parse(&origin(o), body).unwrap() {
            Request::Create(c) => c,
            other => panic!("{other:?}"),
        }
    }

    fn get(o: &str, body: &str) -> Get {
        match parse(&origin(o), body).unwrap() {
            Request::Get(g) => g,
            other => panic!("{other:?}"),
        }
    }

    fn bytes(v: &Value) -> Vec<u8> {
        unb64(v.as_str().unwrap()).unwrap()
    }

    /// What a relying party does with a registration: read the attestation, check the client
    /// data and the flags, and keep the public key.
    fn verify_registration(answer: &Value, rp_id: &str, o: &str) -> (Vec<u8>, Vec<u8>) {
        let response = &answer["response"];
        let client: Value = serde_json::from_slice(&bytes(&response["clientDataJSON"])).unwrap();
        assert_eq!(client["type"], "webauthn.create");
        assert_eq!(client["origin"], o);
        assert_eq!(client["crossOrigin"], false);
        assert_eq!(bytes(&client["challenge"]), b"register-challenge");
        let object = bytes(&response["attestationObject"]);
        let (object, used) = cbor::Value::decode(&object).unwrap();
        assert_eq!(used, bytes(&response["attestationObject"]).len());
        assert_eq!(
            object.get(&cbor::Value::text("fmt")),
            Some(&cbor::Value::text("none"))
        );
        let Some(cbor::Value::Bytes(data)) = object.get(&cbor::Value::text("authData")) else {
            panic!("no authData");
        };
        assert_eq!(data, &bytes(&response["authenticatorData"]));
        assert_eq!(&data[..32], &wire::sha256(rp_id.as_bytes()));
        assert_eq!(data[32], flags::UP | flags::UV | flags::AT);
        assert_eq!(&data[33..37], &[0; 4]);
        assert_eq!(&data[37..53], &wire::AAGUID);
        let len = u16::from_be_bytes([data[53], data[54]]) as usize;
        let id = data[55..55 + len].to_vec();
        assert_eq!(id, bytes(&answer["rawId"]));
        assert_eq!(answer["id"], answer["rawId"]);
        let (key, used) = cbor::Value::decode(&data[55 + len..]).unwrap();
        assert_eq!(55 + len + used, data.len(), "nothing after the key");
        let coord = |label: i64| match key.get(&cbor::Value::Int(label)) {
            Some(cbor::Value::Bytes(b)) => b.clone(),
            other => panic!("{other:?}"),
        };
        let mut point = vec![4u8];
        point.extend(coord(-2));
        point.extend(coord(-3));
        // The key the page reads with getPublicKey() is the same one.
        assert_eq!(&bytes(&response["publicKey"])[26..], &point[..]);
        assert_eq!(response["publicKeyAlgorithm"], -7);
        assert_eq!(response["transports"], json!(["internal"]));
        (id, point)
    }

    /// What a relying party does with an assertion: check the client data and flags, and the
    /// signature over the authenticator data and the client data's hash.
    fn verify_assertion(answer: &Value, rp_id: &str, o: &str, key: &[u8]) {
        let response = &answer["response"];
        let client_raw = bytes(&response["clientDataJSON"]);
        let client: Value = serde_json::from_slice(&client_raw).unwrap();
        assert_eq!(client["type"], "webauthn.get");
        assert_eq!(client["origin"], o);
        assert_eq!(bytes(&client["challenge"]), b"sign-in-challenge");
        let data = bytes(&response["authenticatorData"]);
        assert_eq!(data.len(), 37);
        assert_eq!(&data[..32], &wire::sha256(rp_id.as_bytes()));
        assert_eq!(data[32], flags::UP | flags::UV);
        let mut signed = data.clone();
        signed.extend_from_slice(&wire::sha256(&client_raw));
        let signature = bytes(&response["signature"]);
        assert!(verifies(key, &signed, &signature), "the signature verifies");
        // And over nothing else: not the data alone, and not for another page.
        assert!(!verifies(key, &data, &signature));
        signed[0] ^= 1;
        assert!(!verifies(key, &signed, &signature));
    }

    #[test]
    fn a_passkey_registers_and_then_signs_in_as_a_relying_party_checks_it() {
        let _guard = isolated();
        let store = temp_store("ceremony");
        let o = "https://login.example.com";
        let request = create(
            o,
            &create_body(Some("example.com"), b"user-1", "ada@example.com"),
        );
        assert_eq!(request.rp_id, "example.com");
        assert_eq!(request.user_name, "ada@example.com");
        assert!(!request.already(&store).unwrap());
        let (answer, passkey) = request.finish(&StandIn, &store, 100).unwrap();
        let (id, key) = verify_registration(&answer, "example.com", o);
        assert_eq!(id.len(), 32);
        assert_eq!(answer["type"], "public-key");
        assert_eq!(answer["authenticatorAttachment"], "platform");
        assert_eq!(
            answer["clientExtensionResults"],
            json!({ "credProps": { "rk": true } })
        );
        assert_eq!(store.list().unwrap(), vec![passkey.clone()]);
        assert_eq!(passkey.rp_id, "example.com");
        assert_eq!(passkey.created, 100);

        // Signing in without naming a credential (a discoverable one), from another page of the
        // same site.
        let o2 = "https://www.example.com:8443";
        let request = get(o2, &get_body(Some("example.com"), &[]));
        let candidates = request.candidates(&store).unwrap();
        assert_eq!(candidates, vec![passkey.clone()]);
        let answer = request
            .finish(&candidates[0], &StandIn, &store, 200)
            .unwrap();
        verify_assertion(&answer, "example.com", o2, &key);
        assert_eq!(bytes(&answer["response"]["userHandle"]), b"user-1");
        assert_eq!(bytes(&answer["rawId"]), id);
        assert_eq!(answer["clientExtensionResults"], json!({}));
        assert_eq!(store.list().unwrap()[0].last_used, Some(200));

        // And with the credential named, as a site does after asking for the user name.
        let request = get(o, &get_body(Some("example.com"), &[&passkey.id, "AAAA"]));
        let answer = request.finish(&passkey, &StandIn, &store, 300).unwrap();
        verify_assertion(&answer, "example.com", o, &key);
    }

    #[test]
    fn a_passkey_never_signs_for_another_site() {
        let _guard = isolated();
        let store = temp_store("other-site");
        let (_, passkey) = create("https://example.com", &create_body(None, b"u", "ada"))
            .finish(&StandIn, &store, 1)
            .unwrap();
        assert_eq!(passkey.rp_id, "example.com");

        // Another site can't claim the id: the request is refused before anything is looked up.
        for (o, claimed) in [
            ("https://evil.net", Some("example.com")),
            ("https://example.com.evil.net", Some("example.com")),
            ("https://notexample.com", Some("example.com")),
            ("https://example.com", Some("com")),
            ("http://example.com", None),
            ("https://93.184.216.34", None),
        ] {
            let err = parse(&origin(o), &get_body(claimed, &[])).unwrap_err();
            assert_eq!(err.name, "SecurityError", "{o} as {claimed:?}");
            let err = parse(&origin(o), &create_body(claimed, b"u", "ada")).unwrap_err();
            assert_eq!(err.name, "SecurityError", "{o} as {claimed:?}");
        }

        // Under its own id, another site finds none of example.com's passkeys, even when it
        // names the credential; and the passkey can't be forced through.
        for o in [
            "https://evil.net",
            "https://login.example.com",
            "https://example.org",
        ] {
            let request = get(o, &get_body(None, &[&passkey.id]));
            assert_eq!(request.candidates(&store).unwrap(), vec![], "{o}");
            let err = request.finish(&passkey, &StandIn, &store, 2).unwrap_err();
            assert_eq!(err.name, "NotAllowedError", "{o}");
        }
        // What is signed names the site and the page: the hash of the id, and the origin.
        let request = get(
            "https://login.example.com",
            &get_body(Some("example.com"), &[]),
        );
        let answer = request.finish(&passkey, &StandIn, &store, 3).unwrap();
        let client: Value =
            serde_json::from_slice(&bytes(&answer["response"]["clientDataJSON"])).unwrap();
        assert_eq!(client["origin"], "https://login.example.com");
    }

    #[test]
    fn only_the_credentials_a_site_names_can_answer_it() {
        let _guard = isolated();
        let store = temp_store("allow");
        let o = "https://example.com";
        let (_, ada) = create(o, &create_body(None, b"u1", "ada"))
            .finish(&StandIn, &store, 1)
            .unwrap();
        let (_, bob) = create(o, &create_body(None, b"u2", "bob"))
            .finish(&StandIn, &store, 2)
            .unwrap();
        let ids = |g: &Get| -> Vec<String> {
            g.candidates(&store)
                .unwrap()
                .into_iter()
                .map(|p| p.user_name)
                .collect()
        };
        assert_eq!(ids(&get(o, &get_body(None, &[]))), ["ada", "bob"]);
        assert_eq!(ids(&get(o, &get_body(None, &[&bob.id]))), ["bob"]);
        assert_eq!(
            ids(&get(o, &get_body(None, &["AAAA"]))),
            Vec::<String>::new()
        );
        // A passkey outside the list isn't signed with, whatever is passed in.
        let err = get(o, &get_body(None, &[&bob.id]))
            .finish(&ada, &StandIn, &store, 3)
            .unwrap_err();
        assert_eq!(err.name, "NotAllowedError");
        // A descriptor of another type is ignored, as the specification says: beside one of
        // ours it changes nothing, and a list of nothing else names no passkey here (which is
        // not the same as naming none at all).
        let mut body: Value = serde_json::from_str(&get_body(None, &[&bob.id, &ada.id])).unwrap();
        body["publicKey"]["allowCredentials"][0]["type"] = json!("something-new");
        assert_eq!(ids(&get(o, &body.to_string())), ["ada"]);
        body["publicKey"]["allowCredentials"][1]["type"] = json!("something-new");
        assert_eq!(ids(&get(o, &body.to_string())), Vec::<String>::new());
        body["publicKey"]["allowCredentials"] = json!([]);
        assert_eq!(ids(&get(o, &body.to_string())), ["ada", "bob"]);
    }

    #[test]
    fn a_site_that_excludes_a_passkey_kept_here_gets_invalid_state() {
        let _guard = isolated();
        let store = temp_store("exclude");
        let o = "https://example.com";
        let (_, first) = create(o, &create_body(None, b"u1", "ada"))
            .finish(&StandIn, &store, 1)
            .unwrap();
        let mut body: Value = serde_json::from_str(&create_body(None, b"u1", "ada")).unwrap();
        body["publicKey"]["excludeCredentials"] =
            json!([{ "type": "public-key", "id": first.id, "transports": ["internal"] }]);
        let again = create(o, &body.to_string());
        assert!(again.already(&store).unwrap());
        assert_eq!(
            again.finish(&StandIn, &store, 2).unwrap_err().name,
            "InvalidStateError"
        );
        assert_eq!(store.list().unwrap().len(), 1);
        // Another site listing the same id excludes nothing of its own.
        let elsewhere = create("https://example.org", &body.to_string());
        assert!(!elsewhere.already(&store).unwrap());
    }

    #[test]
    fn a_new_passkey_takes_the_old_ones_place_only_once_its_page_has_it() {
        let _guard = isolated();
        let store = temp_store("settle");
        let o = "https://example.com";
        let request = create(o, &create_body(None, b"u1", "ada"));
        assert!(!request.replaces(&store).unwrap());
        let (_, first) = request.finish(&StandIn, &store, 1).unwrap();
        settle(&StandIn, &store, &first, true);
        assert_eq!(store.list().unwrap(), vec![first.clone()]);
        // A second one for the account, whose page went away before the answer came: the site
        // never had it, so it goes and the first still signs in.
        assert!(request.replaces(&store).unwrap());
        let (_, lost) = request.finish(&StandIn, &store, 2).unwrap();
        assert_eq!(store.list().unwrap().len(), 2, "both, until it's known");
        settle(&StandIn, &store, &lost, false);
        assert_eq!(store.list().unwrap(), vec![first.clone()]);
        let sign_in = get(o, &get_body(None, &[]));
        assert!(sign_in.finish(&first, &StandIn, &store, 3).is_ok());
        // And one whose page got it: now the first goes, and another account's stays.
        let (_, other) = create(o, &create_body(None, b"u2", "bob"))
            .finish(&StandIn, &store, 4)
            .unwrap();
        settle(&StandIn, &store, &other, true);
        let (_, second) = request.finish(&StandIn, &store, 5).unwrap();
        settle(&StandIn, &store, &second, true);
        assert_eq!(store.list().unwrap(), vec![other, second]);
    }

    #[test]
    fn nothing_is_made_or_signed_when_the_user_says_no() {
        let _guard = isolated();
        let store = temp_store("cancel");
        let o = "https://example.com";
        set_stand_in(false);
        let err = create(o, &create_body(None, b"u1", "ada"))
            .finish(&StandIn, &store, 1)
            .unwrap_err();
        assert_eq!(err.name, "NotAllowedError");
        assert_eq!(store.list().unwrap(), vec![]);
        set_stand_in(true);
        let (_, passkey) = create(o, &create_body(None, b"u1", "ada"))
            .finish(&StandIn, &store, 1)
            .unwrap();
        set_stand_in(false);
        let err = get(o, &get_body(None, &[]))
            .finish(&passkey, &StandIn, &store, 2)
            .unwrap_err();
        assert_eq!(err.name, "NotAllowedError");
        assert_eq!(store.list().unwrap()[0].last_used, None);
        set_stand_in(true);
    }

    #[test]
    fn requests_that_arent_webauthns_are_type_errors() {
        let o = origin("https://example.com");
        let name = |body: Value| parse(&o, &body.to_string()).unwrap_err().name;
        let good: Value = serde_json::from_str(&create_body(None, b"u1", "ada")).unwrap();
        assert!(parse(&o, &good.to_string()).is_ok());
        let without = |path: &[&str]| {
            let mut body = good.clone();
            let (last, parents) = path.split_last().unwrap();
            let mut at = &mut body["publicKey"];
            for p in parents {
                at = &mut at[*p];
            }
            at.as_object_mut().unwrap().remove(*last);
            body
        };
        for path in [
            &["rp"][..],
            &["rp", "name"],
            &["user"],
            &["user", "id"],
            &["user", "name"],
            &["user", "displayName"],
            &["challenge"],
            &["pubKeyCredParams"],
        ] {
            assert_eq!(name(without(path)), "TypeError", "{path:?}");
        }
        let with = |path: &str, value: Value| {
            let mut body = good.clone();
            *body.pointer_mut(path).unwrap() = value;
            body
        };
        assert_eq!(name(with("/publicKey/user/id", json!(""))), "TypeError");
        assert_eq!(
            name(with("/publicKey/user/id", json!(b64(&[1u8; 65])))),
            "TypeError"
        );
        assert_eq!(
            name(with("/publicKey/user/id", json!("not base64!"))),
            "TypeError"
        );
        assert_eq!(name(with("/publicKey/challenge", json!(7))), "TypeError");
        assert_eq!(
            name(with("/publicKey/rp", json!("example.com"))),
            "TypeError"
        );
        assert_eq!(name(with("/kind", json!("store"))), "TypeError");
        assert_eq!(name(json!({ "kind": "get" })), "TypeError");
        assert_eq!(parse(&o, "not json").unwrap_err().name, "TypeError");
        assert_eq!(
            parse(&o, &format!("\"{}\"", "a".repeat(MAX_REQUEST)))
                .unwrap_err()
                .name,
            "TypeError"
        );
        // 64 bytes is the most a user handle may be.
        assert!(
            parse(
                &o,
                &with("/publicKey/user/id", json!(b64(&[1u8; 64]))).to_string()
            )
            .is_ok()
        );
    }

    #[test]
    fn a_site_gets_only_what_trek_can_be() {
        let o = origin("https://example.com");
        let good: Value = serde_json::from_str(&create_body(None, b"u1", "ada")).unwrap();
        let with = |path: &str, value: Value| {
            let mut body = good.clone();
            *body.pointer_mut(path).unwrap() = value;
            parse(&o, &body.to_string())
        };
        // No ES256 among the algorithms.
        let err = with(
            "/publicKey/pubKeyCredParams",
            json!([{ "type": "public-key", "alg": -257 }, { "type": "other", "alg": -7 }]),
        )
        .unwrap_err();
        assert_eq!(err.name, "NotSupportedError");
        // An empty list is the specification's defaults.
        assert!(with("/publicKey/pubKeyCredParams", json!([])).is_ok());
        // A security key was asked for.
        let err = with(
            "/publicKey/authenticatorSelection",
            json!({ "authenticatorAttachment": "cross-platform" }),
        )
        .unwrap_err();
        assert_eq!(err.name, "NotAllowedError");
        assert!(
            with(
                "/publicKey/authenticatorSelection",
                json!({ "authenticatorAttachment": "platform", "residentKey": "discouraged" }),
            )
            .is_ok()
        );
        // A passkey made on the quiet (conditional create) is refused, and so is anything asked
        // for silently: nothing here happens without the user being asked.
        for (kind, mediation) in [
            ("create", "conditional"),
            ("create", "silent"),
            ("get", "silent"),
        ] {
            let mut quiet = if kind == "create" {
                good.clone()
            } else {
                serde_json::from_str(&get_body(None, &[])).unwrap()
            };
            quiet["mediation"] = json!(mediation);
            assert_eq!(
                parse(&o, &quiet.to_string()).unwrap_err().name,
                "NotAllowedError",
                "{kind} {mediation}"
            );
        }
        // The old U2F extension is answered "not used".
        let mut body: Value = serde_json::from_str(&get_body(None, &[])).unwrap();
        body["publicKey"]["extensions"] = json!({ "appid": "https://example.com/u2f" });
        body["mediation"] = json!("conditional");
        let Request::Get(request) = parse(&o, &body.to_string()).unwrap() else {
            panic!()
        };
        assert!(request.appid && request.conditional);
    }

    #[test]
    fn the_account_in_a_sentence() {
        assert_eq!(account_words("ada@example.com", "Ada"), "ada@example.com");
        assert_eq!(account_words(" ", "Ada"), "Ada");
        assert_eq!(account_words("", ""), "your account");
    }
}
