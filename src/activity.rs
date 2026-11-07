//! Durable attribution of native user input without storing its plaintext.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub const HELD_PHONE_SECS: u64 = 30 * 60;

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

/// Terminal delivery is not native acceptance. Keep an independent, private
/// receipt until an unambiguous new native input has the submitted fingerprint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Submission {
    pub receipt: String,
    before: Option<Vec<String>>,
    fingerprint: Option<String>,
    pub submitted_at: u64,
    pub warned: bool,
}

impl Submission {
    pub fn new(receipt: &str, text: &str, inputs: Option<&[UserInput]>, at: u64) -> Self {
        Self {
            receipt: receipt.into(),
            before: inputs.map(|inputs| inputs.iter().map(|input| input.id.clone()).collect()),
            fingerprint: UserInput::new(String::new(), text).fingerprint,
            submitted_at: at,
            warned: false,
        }
    }

    pub fn overdue(&self, at: u64) -> bool {
        at.saturating_sub(self.submitted_at) >= 60
    }

    pub fn observe(submissions: &mut Vec<Self>, inputs: &[UserInput]) {
        // Match the whole group: one echo must not confirm two equal submissions,
        // nor may two equal native inputs be attributed to one phone receipt.
        let accepted: Vec<_> = submissions
            .iter()
            .enumerate()
            .filter_map(|(index, submission)| {
                let fingerprint = submission.fingerprint.as_ref()?;
                let peers: Vec<_> = submissions
                    .iter()
                    .filter(|other| other.fingerprint.as_ref() == Some(fingerprint))
                    .collect();
                let mut ids = HashSet::new();
                let echoes: Vec<_> = inputs
                    .iter()
                    .filter(|input| {
                        input.complete
                            && input.fingerprint.as_ref() == Some(fingerprint)
                            && peers.iter().any(|peer| {
                                peer.before
                                    .as_ref()
                                    .is_some_and(|before| !before.contains(&input.id))
                            })
                            && ids.insert(input.id.as_str())
                    })
                    .collect();
                if echoes.len() != peers.len() {
                    return None;
                }
                let candidates: Vec<Vec<usize>> = peers
                    .iter()
                    .map(|peer| {
                        echoes
                            .iter()
                            .enumerate()
                            .filter(|(_, input)| {
                                peer.before
                                    .as_ref()
                                    .is_some_and(|before| !before.contains(&input.id))
                            })
                            .map(|(index, _)| index)
                            .collect()
                    })
                    .collect();
                let mut owners = vec![None; echoes.len()];
                (0..peers.len())
                    .all(|peer| {
                        assign_native_input(
                            peer,
                            &candidates,
                            &mut owners,
                            &mut vec![false; echoes.len()],
                        )
                    })
                    .then_some(index)
            })
            .collect();
        let mut index = 0;
        submissions.retain(|_| {
            let keep = !accepted.contains(&index);
            index += 1;
            keep
        });
    }
}

// Each receipt must own a different eligible native message. Reassign an earlier
// match if a later receipt has a narrower baseline (for example, an incomplete echo).
fn assign_native_input(
    receipt: usize,
    candidates: &[Vec<usize>],
    owners: &mut [Option<usize>],
    seen: &mut [bool],
) -> bool {
    for &input in &candidates[receipt] {
        if seen[input] {
            continue;
        }
        seen[input] = true;
        if owners[input]
            .is_none_or(|previous| assign_native_input(previous, candidates, owners, seen))
        {
            owners[input] = Some(receipt);
            return true;
        }
    }
    false
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct PhoneInput {
    receipt: String,
    before: Vec<String>,
    fingerprint: Option<String>,
    confirmed: bool,
    #[serde(default)]
    held: bool,
    #[serde(default)]
    held_at: u64,
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
        let mut consumed: Vec<String> = Vec::new();
        // Inputs already matched to a phone receipt. Identical sends match one to one.
        let mut matched: Vec<&str> = Vec::new();
        let mut local = false;
        for input in &fresh {
            let candidates: Vec<_> = self
                .phone
                .iter()
                .filter(|phone| {
                    phone.confirmed
                        && input.fingerprint.is_some()
                        && input.fingerprint == phone.fingerprint
                        && !phone.before.contains(&input.id)
                        && !consumed.contains(&phone.receipt)
                })
                .collect();
            // Held prompts run in send order, so identical held text matches the oldest submission.
            let receipt = match candidates.first() {
                Some(first) if candidates.iter().all(|phone| phone.held) => Some(first),
                // Without holds, every identical fresh input needs its own receipt, or the text is ambiguous.
                Some(first)
                    if candidates.len()
                        == fresh
                            .iter()
                            .filter(|other| {
                                other.fingerprint == first.fingerprint
                                    && !first.before.contains(&other.id)
                                    && !matched.contains(&other.id.as_str())
                            })
                            .count() =>
                {
                    Some(first)
                }
                _ => None,
            }
            .map(|phone| phone.receipt.clone());
            let Some(receipt) = receipt else {
                local |= !input.queued;
                continue;
            };
            consumed.push(receipt);
            matched.push(&input.id);
            if let Some(turn) = &input.turn
                && !self.phone_turns.contains(turn)
            {
                self.phone_turns.push(turn.clone());
            }
        }
        // A submission can explain only the first new input batch after its checkpoint.
        // A held submission waits behind earlier queued prompts. A later match proves it was dropped.
        let last = self
            .phone
            .iter()
            .rposition(|phone| consumed.contains(&phone.receipt));
        let mut index = 0;
        self.phone.retain(|phone| {
            index += 1;
            !consumed.contains(&phone.receipt)
                && last.is_none_or(|last| index > last + 1)
                // Unmatched local input makes an unheld submission stale. Matched echoes of
                // other submissions do not, so staggered echoes keep the later receipt.
                && (phone.held || !local)
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
        self.phone.iter().any(|phone| phone.confirmed) || self.phone_open
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
        self.start_phone_current(receipt, text);
    }

    pub fn start_phone_current(&mut self, receipt: &str, text: &str) {
        let before = self.observed.get_or_insert_with(Vec::new).clone();
        self.phone.push(PhoneInput {
            receipt: receipt.into(),
            before,
            fingerprint: UserInput::new(String::new(), text).fingerprint,
            confirmed: false,
            held: false,
            held_at: 0,
        });
    }

    pub fn confirm_phone(&mut self, receipt: &str) {
        for phone in &mut self.phone {
            if phone.receipt == receipt {
                phone.confirmed = true;
            }
        }
    }

    /// Keeps the submission across unrelated input until its own prompt appears.
    pub fn hold_phone(&mut self, receipt: &str, now: u64) {
        for phone in &mut self.phone {
            if phone.receipt == receipt {
                phone.held = true;
                phone.held_at = now;
            }
        }
    }

    /// Returns how many confirmed submissions expired.
    /// A held entry saved without `held_at` starts its limit at the first check.
    pub fn expire_held(&mut self, now: u64) -> usize {
        for phone in self
            .phone
            .iter_mut()
            .filter(|phone| phone.held && phone.held_at == 0)
        {
            phone.held_at = now;
        }
        let before = self.phone.iter().filter(|phone| phone.confirmed).count();
        self.phone
            .retain(|phone| !phone.held || now.saturating_sub(phone.held_at) < HELD_PHONE_SECS);
        before - self.phone.iter().filter(|phone| phone.confirmed).count()
    }

    pub fn cancel_phone(&mut self, receipt: &str) {
        self.phone.retain(|phone| phone.receipt != receipt);
    }
}
