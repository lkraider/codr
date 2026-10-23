//! Durable attribution of native user input without storing its plaintext.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub struct UserInput {
    pub id: String,
    pub fingerprint: Option<String>,
    /// The turn whose final reply answers this input.
    pub turn: Option<String>,
    /// Delivered into a running turn. It never opens a turn and never mutes forwarding.
    pub queued: bool,
    complete: bool,
}

impl UserInput {
    pub fn new(id: String, text: &str) -> Self {
        let text = text.replace("\r\n", "\n");
        Self {
            id,
            complete: true,
            fingerprint: (!text.trim().is_empty())
                .then(|| hex::encode(Sha256::digest(text.trim().as_bytes()))),
            turn: None,
            queued: false,
        }
    }

    pub fn pending(id: String) -> Self {
        Self {
            id,
            fingerprint: None,
            turn: None,
            queued: false,
            complete: false,
        }
    }

    pub fn in_turn(mut self, turn: Option<String>) -> Self {
        self.turn = turn;
        self
    }

    pub fn queued(mut self) -> Self {
        self.queued = true;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct PhoneInput {
    receipt: String,
    before: Vec<String>,
    fingerprint: Option<String>,
    confirmed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Activity {
    observed: Option<Vec<String>>,
    phone: Vec<PhoneInput>,
    #[serde(default)]
    unresolved: bool,
    #[serde(default)]
    phone_turns: Vec<String>,
    #[serde(default)]
    phone_open: bool,
}

impl Activity {
    /// First observation is a migration baseline, never evidence of fresh activity.
    pub fn observe(&mut self, inputs: &[UserInput]) -> bool {
        let Some(observed) = &mut self.observed else {
            self.observed = Some(inputs.iter().map(|input| input.id.clone()).collect());
            return false;
        };
        let known: HashSet<_> = observed.iter().cloned().collect();
        self.unresolved = inputs
            .iter()
            .any(|input| !input.complete && !known.contains(&input.id));
        let fresh: Vec<_> = inputs
            .iter()
            .filter(|input| input.complete && !known.contains(&input.id))
            .collect();
        let phone = &self.phone;
        let matched = |input: &UserInput| {
            let candidates: Vec<_> = phone
                .iter()
                .filter(|phone| {
                    phone.confirmed
                        && input.fingerprint.is_some()
                        && input.fingerprint == phone.fingerprint
                        && !phone.before.contains(&input.id)
                })
                .collect();
            (candidates.len() == 1
                && fresh
                    .iter()
                    .filter(|other| {
                        other.fingerprint == candidates[0].fingerprint
                            && !candidates[0].before.contains(&other.id)
                    })
                    .count()
                    == 1)
                .then(|| candidates[0].receipt.clone())
        };
        let local = fresh
            .iter()
            .any(|input| !input.queued && matched(input).is_none());
        let mut consumed = Vec::new();
        for input in &fresh {
            if let Some(receipt) = matched(input) {
                consumed.push(receipt);
                if let Some(turn) = &input.turn
                    && !self.phone_turns.contains(turn)
                {
                    self.phone_turns.push(turn.clone());
                }
            }
        }
        // A submission can explain only the first new input batch after its checkpoint.
        self.phone.retain(|phone| {
            !consumed.contains(&phone.receipt)
                && !fresh
                    .iter()
                    .any(|input| !input.queued && !phone.before.contains(&input.id))
        });
        observed.extend(fresh.into_iter().map(|input| input.id.clone()));
        local
    }

    pub fn baseline(&mut self, inputs: &[UserInput]) {
        let _ = self.observe(inputs);
    }

    pub fn phone_turn(&self, turn: &str) -> bool {
        self.phone_turns.iter().any(|known| known == turn)
    }

    /// True from a phone submission until the final reply of its turn.
    pub fn awaiting_phone(&self) -> bool {
        !self.phone.is_empty() || self.phone_open
    }

    pub fn track_latest(&mut self, latest: Option<&str>, answered: bool) {
        self.phone_open = !answered && latest.is_some_and(|turn| self.phone_turn(turn));
    }

    pub fn retain_turns(&mut self, live: &[String]) {
        self.phone_turns.retain(|turn| live.contains(turn));
    }

    pub fn unresolved(&self) -> bool {
        self.unresolved
    }

    /// An ended session cannot complete attribution reliably. The caller mutes first.
    pub fn retire(&mut self) {
        self.unresolved = false;
        self.phone.clear();
    }

    pub fn start_phone(&mut self, receipt: &str, text: &str, inputs: &[UserInput]) {
        self.baseline(inputs);
        self.phone.push(PhoneInput {
            receipt: receipt.into(),
            before: inputs.iter().map(|input| input.id.clone()).collect(),
            fingerprint: UserInput::new(String::new(), text).fingerprint,
            confirmed: false,
        });
    }

    pub fn confirm_phone(&mut self, receipt: &str) {
        for phone in &mut self.phone {
            if phone.receipt == receipt {
                phone.confirmed = true;
            }
        }
    }

    pub fn cancel_phone(&mut self, receipt: &str) {
        self.phone.retain(|phone| phone.receipt != receipt);
    }
}
