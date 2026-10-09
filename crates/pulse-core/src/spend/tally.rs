// Ported from upstream Sources/Pulse/Usage/UsageLedger.swift (TokenTally, ReplyTiming) and TokenCost.swift.
//! Tokens of each kind, what they cost, and how quickly replies came back.

use std::collections::HashMap;
use std::ops::{Add, AddAssign};

use serde::{Deserialize, Serialize};

use super::prices::ModelPrice;

fn is_zero(value: &i64) -> bool {
    *value == 0
}

/// Money by kind of token, at one model's own rates. The split is the summand a day's or an
/// agent's money is rolled up from, so there is one arithmetic behind every figure.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TokenCost {
    pub input: f64,
    pub cache_write: f64,
    pub cache_read: f64,
    pub output: f64,
}

impl TokenCost {
    pub fn total(&self) -> f64 {
        self.input + self.cache_write + self.cache_read + self.output
    }
}

impl Add for TokenCost {
    type Output = TokenCost;
    fn add(self, rhs: TokenCost) -> TokenCost {
        TokenCost {
            input: self.input + rhs.input,
            cache_write: self.cache_write + rhs.cache_write,
            cache_read: self.cache_read + rhs.cache_read,
            output: self.output + rhs.output,
        }
    }
}

impl AddAssign for TokenCost {
    fn add_assign(&mut self, rhs: TokenCost) {
        *self = *self + rhs;
    }
}

/// Tokens of each kind, which is what a price list needs to become money.
///
/// Serialised sparsely (only what is not zero) and every field is optional on the way in, so a
/// cache written before a field existed still reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TokenTally {
    /// Fresh input: what was not served from the prompt cache.
    #[serde(skip_serializing_if = "is_zero")]
    pub input: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub cache_write: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub cache_read: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub output: i64,
    /// The part of `cache_write` held for an hour rather than five minutes: inside `cache_write`,
    /// not beside it, so it is not in `total()`.
    #[serde(skip_serializing_if = "is_zero")]
    pub cache_write_1h: i64,
    /// Replies whose usage carried no cache field at all (absent, not zero).
    #[serde(skip_serializing_if = "is_zero")]
    pub replies_without_cache_fields: i64,
    /// The same tokens again for requests whose context was over a size, keyed by the largest of
    /// `CONTEXT_BOUNDARIES` it passed. A subset of this tally, not beside it.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub context_bands: HashMap<i64, TokenTally>,
}

/// The context sizes models.dev prices a tier from that a band is kept for: 128K, 200K, 256K, and
/// OpenAI's 272K. A tier at a size between two of these applies from the next one up, never early.
pub const CONTEXT_BOUNDARIES: [i64; 4] = [128_000, 200_000, 256_000, 272_000];

/// A one-hour cache write against the plain input rate, as Anthropic publishes it. models.dev
/// lists only the five-minute rate.
pub const HOUR_WRITE_MULTIPLIER: f64 = 2.0;

impl TokenTally {
    pub fn new(input: i64, cache_write: i64, cache_read: i64, output: i64) -> Self {
        Self { input, cache_write, cache_read, output, ..Self::default() }
    }

    /// This tally, marked as one request whose context was `context` tokens.
    pub fn request(&self, context: i64) -> TokenTally {
        let Some(band) = CONTEXT_BOUNDARIES.iter().rev().find(|&&b| context > b).copied() else {
            return self.clone();
        };
        let mut plain = self.clone();
        plain.context_bands = HashMap::new();
        let mut copy = self.clone();
        copy.context_bands = HashMap::from([(band, plain)]);
        copy
    }

    pub fn total(&self) -> i64 {
        self.input + self.cache_write + self.cache_read + self.output
    }

    /// Each kind at the larger of the two: the most of it either held.
    pub fn highest(&self, other: &TokenTally) -> TokenTally {
        let mut bands = self.context_bands.clone();
        for (band, tally) in &other.context_bands {
            let merged = bands.get(band).map_or_else(|| tally.clone(), |mine| mine.highest(tally));
            bands.insert(*band, merged);
        }
        TokenTally {
            input: self.input.max(other.input),
            cache_write: self.cache_write.max(other.cache_write),
            cache_read: self.cache_read.max(other.cache_read),
            output: self.output.max(other.output),
            cache_write_1h: self.cache_write_1h.max(other.cache_write_1h),
            replies_without_cache_fields: self.replies_without_cache_fields.max(other.replies_without_cache_fields),
            context_bands: bands,
        }
    }

    /// What this holds beyond another tally, kind by kind and never below zero.
    pub fn beyond(&self, other: &TokenTally) -> TokenTally {
        let mut bands = HashMap::new();
        for (band, tally) in &self.context_bands {
            let more = tally.beyond(other.context_bands.get(band).unwrap_or(&TokenTally::default()));
            if more.total() > 0 {
                bands.insert(*band, more);
            }
        }
        TokenTally {
            input: (self.input - other.input).max(0),
            cache_write: (self.cache_write - other.cache_write).max(0),
            cache_read: (self.cache_read - other.cache_read).max(0),
            output: (self.output - other.output).max(0),
            cache_write_1h: (self.cache_write_1h - other.cache_write_1h).max(0),
            replies_without_cache_fields: (self.replies_without_cache_fields - other.replies_without_cache_fields).max(0),
            context_bands: bands,
        }
    }

    /// Every reply behind this tally said nothing about the cache, and none was read or written:
    /// there is no cache figure, not a zero one.
    pub fn reports_no_cache(&self) -> bool {
        self.replies_without_cache_fields > 0 && self.cache_read == 0 && self.cache_write == 0
    }

    /// Everything sent that was not read back from the cache.
    pub fn fresh(&self) -> i64 {
        self.input + self.cache_write
    }

    /// Whether the recorded kinds plus an explicit unclassified count account for the reported
    /// total. Missing detail is never inferred by subtraction.
    pub fn accounts_for(&self, tokens: i64, unclassified: i64) -> bool {
        let mut total: i64 = 0;
        for value in [self.input, self.cache_write, self.cache_read, self.output, unclassified] {
            if value < 0 {
                return false;
            }
            match total.checked_add(value) {
                Some(sum) => total = sum,
                None => return false,
            }
        }
        total == tokens
    }

    /// The sum of the four kinds, or None when any is negative or the sum overflows.
    pub fn checked_total(&self) -> Option<i64> {
        let mut total: i64 = 0;
        for value in [self.input, self.cache_write, self.cache_read, self.output] {
            if value < 0 {
                return None;
            }
            total = total.checked_add(value)?;
        }
        Some(total)
    }

    /// Both tallies added without overflowing, or None.
    pub fn checked_add(&self, other: &TokenTally) -> Option<TokenTally> {
        Some(TokenTally {
            input: self.input.checked_add(other.input)?,
            cache_write: self.cache_write.checked_add(other.cache_write)?,
            cache_read: self.cache_read.checked_add(other.cache_read)?,
            output: self.output.checked_add(other.output)?,
            cache_write_1h: self.cache_write_1h.checked_add(other.cache_write_1h)?,
            replies_without_cache_fields: self
                .replies_without_cache_fields
                .checked_add(other.replies_without_cache_fields)?,
            context_bands: HashMap::new(),
        })
    }

    /// The token kinds less another tally's, for taking a band out of the whole before the rest
    /// is priced at the base rates.
    fn removing(&self, other: &TokenTally) -> TokenTally {
        TokenTally {
            input: (self.input - other.input).max(0),
            cache_write: (self.cache_write - other.cache_write).max(0),
            cache_read: (self.cache_read - other.cache_read).max(0),
            output: (self.output - other.output).max(0),
            cache_write_1h: (self.cache_write_1h - other.cache_write_1h).max(0),
            replies_without_cache_fields: 0,
            context_bands: HashMap::new(),
        }
    }

    /// Rates are per million tokens; a missing cache rate falls back to the plain input rate.
    ///
    /// The split is the only formula: `cost_at` is this breakdown's total. A long-context request
    /// is priced at its tier, whole; the rest at the base rates.
    pub fn cost_breakdown(&self, price: &ModelPrice) -> TokenCost {
        let mut rest = self.clone();
        rest.context_bands = HashMap::new();
        let mut cost = TokenCost::default();
        for (band, requests) in &self.context_bands {
            let Some(tier) = price.tier_for_band(*band) else { continue };
            rest = rest.removing(requests);
            cost += requests.flat_cost(&tier.price());
        }
        cost + rest.flat_cost(price)
    }

    fn flat_cost(&self, price: &ModelPrice) -> TokenCost {
        let hour_writes = self.cache_write_1h.max(0).min(self.cache_write);
        TokenCost {
            input: self.input as f64 * price.input / 1_000_000.0,
            cache_write: ((self.cache_write - hour_writes) as f64 * price.cache_write.unwrap_or(price.input)
                + hour_writes as f64 * price.input * HOUR_WRITE_MULTIPLIER)
                / 1_000_000.0,
            cache_read: self.cache_read as f64 * price.cache_read.unwrap_or(price.input) / 1_000_000.0,
            output: self.output as f64 * price.output / 1_000_000.0,
        }
    }

    pub fn cost_at(&self, price: &ModelPrice) -> f64 {
        self.cost_breakdown(price).total()
    }
}

impl Add for TokenTally {
    type Output = TokenTally;
    fn add(mut self, rhs: TokenTally) -> TokenTally {
        self += &rhs;
        self
    }
}

impl Add<&TokenTally> for TokenTally {
    type Output = TokenTally;
    fn add(mut self, rhs: &TokenTally) -> TokenTally {
        self += rhs;
        self
    }
}

impl AddAssign<&TokenTally> for TokenTally {
    fn add_assign(&mut self, rhs: &TokenTally) {
        self.input += rhs.input;
        self.cache_write += rhs.cache_write;
        self.cache_read += rhs.cache_read;
        self.output += rhs.output;
        self.cache_write_1h += rhs.cache_write_1h;
        self.replies_without_cache_fields += rhs.replies_without_cache_fields;
        for (band, tally) in &rhs.context_bands {
            *self.context_bands.entry(*band).or_default() += tally;
        }
    }
}

impl AddAssign for TokenTally {
    fn add_assign(&mut self, rhs: TokenTally) {
        *self += &rhs;
    }
}

impl std::iter::Sum for TokenTally {
    fn sum<I: Iterator<Item = TokenTally>>(iter: I) -> TokenTally {
        iter.fold(TokenTally::default(), |acc, item| acc + item)
    }
}

/// How long replies took to come back, and how much they wrote: what an output speed is worked
/// out from.
///
/// Request sent to reply finished, the only span both CLIs' logs bracket. Only replies long
/// enough for the first-token wait not to dominate are counted (`MINIMUM_OUTPUT`), and none that
/// took longer than `LONGEST`. The first-token wait is Codex's own `time_to_first_token_ms`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReplyTiming {
    pub output_tokens: i64,
    pub seconds: f64,
    pub replies: i64,
    pub first_token_seconds: f64,
    pub first_token_turns: i64,
}

impl ReplyTiming {
    pub const MINIMUM_OUTPUT: i64 = 100;
    /// Ten minutes, in seconds.
    pub const LONGEST: f64 = 10.0 * 60.0;
    /// A first token later than this is a stall or a retry, not a latency.
    pub const LONGEST_FIRST_TOKEN: f64 = 2.0 * 60.0;

    /// One turn's wait for its first token, as the CLI measured it.
    pub fn first_token(after_seconds: f64) -> Option<ReplyTiming> {
        if after_seconds > 0.0 && after_seconds <= Self::LONGEST_FIRST_TOKEN {
            Some(ReplyTiming { first_token_seconds: after_seconds, first_token_turns: 1, ..Self::default() })
        } else {
            None
        }
    }

    /// One reply, or nothing when it cannot stand for a speed.
    pub fn reply(output: i64, seconds: f64) -> Option<ReplyTiming> {
        if output >= Self::MINIMUM_OUTPUT && seconds > 0.0 && seconds <= Self::LONGEST {
            Some(ReplyTiming { output_tokens: output, seconds, replies: 1, ..Self::default() })
        } else {
            None
        }
    }
}

impl Add for ReplyTiming {
    type Output = ReplyTiming;
    fn add(self, rhs: ReplyTiming) -> ReplyTiming {
        ReplyTiming {
            output_tokens: self.output_tokens + rhs.output_tokens,
            seconds: self.seconds + rhs.seconds,
            replies: self.replies + rhs.replies,
            first_token_seconds: self.first_token_seconds + rhs.first_token_seconds,
            first_token_turns: self.first_token_turns + rhs.first_token_turns,
        }
    }
}
