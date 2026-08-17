//! Utility function to stall an iterator

use crate::doc::DetRng;

/// Derive a generator seed from a stall probability.
///
/// Two streams stalled at *different* rates in the same test therefore
/// get different sequences for free, which is the common case. Both
/// halves of the float are folded in so that values sharing a mantissa
/// (`0.25` and `0.5`, whose low 32 bits are both zero) do not collide.
pub(crate) fn seed_for(prob: f64) -> u32 {
    let bits = prob.to_bits();
    (bits as u32) ^ ((bits >> 32) as u32) ^ 0x5F37_59DF
}

/// The [stalling] function wraps an iterator into one that "stalls",
/// returning either `Some(t)` (where `t` is the value yielded by the underlying iterator)
/// or `None`.  The probability of a stall is a parameter `prob`.
///
/// The stall pattern is irregular but **reproducible**: it comes from a
/// [`DetRng`] seeded from `prob`, not from `rand::random`. Tests built on
/// it are deterministic per CLAUDE.md §12 rule 10, so a failure can be
/// re-run rather than chased.
///
/// Irregularity is the point, and is what distinguishes this from
/// [`super::sinks::stalling_periodic`]: a fixed cadence can alias against
/// a widget's own period and hide a bug that only appears at some other
/// phase. Prefer `stalling` when you want unstructured backpressure, and
/// `stalling_periodic` when the test's claim depends on a known rate.
///
/// Two streams in one test that share a probability will share a
/// sequence — see [`stalling_with_seed`] when that matters.
pub fn stalling<S>(s: S, prob: f64) -> impl Iterator<Item = Option<<S as Iterator>::Item>>
where
    S: Iterator,
{
    stalling_with_seed(s, prob, seed_for(prob))
}

/// [`stalling`], with the generator seed given explicitly.
///
/// Needed when one test stalls **two streams at the same probability**.
/// The default seed is a function of `prob` alone, so those two streams
/// would otherwise stall on identical cycles — and two channels that
/// stall in lockstep never exercise the case where one is blocked while
/// the other flows, which for a request/response pair is the interesting
/// one.
pub fn stalling_with_seed<S>(
    mut s: S,
    prob: f64,
    seed: u32,
) -> impl Iterator<Item = Option<<S as Iterator>::Item>>
where
    S: Iterator,
{
    assert!(
        prob < 1.0,
        "Stalling with probability >= 1.0 is not supported, and probably not what you mean"
    );
    let percent = (prob * 100.0).round() as u32;
    let mut det = DetRng::new(seed);
    std::iter::from_fn(move || {
        Some(if det.chance(percent) {
            // Stall the generator and return None
            None
        } else {
            s.next()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(prob: f64) -> Vec<i32> {
        stalling(0..40, prob)
            .take(40)
            .map(|x| x.map(|v| v as i32).unwrap_or(-1))
            .collect()
    }

    /// The whole point: same inputs, same sequence, every run.
    #[test]
    fn stalling_is_reproducible() {
        assert_eq!(pattern(0.23), pattern(0.23));
    }

    /// It must actually stall, and must still deliver the underlying
    /// items in order. A "deterministic" stall that never stalls would
    /// pass the test above while testing nothing.
    #[test]
    fn stalling_stalls_and_preserves_order() {
        let p = pattern(0.23);
        assert!(p.contains(&-1), "must produce stalls");
        let delivered: Vec<i32> = p.iter().copied().filter(|v| *v >= 0).collect();
        assert!(delivered.len() > 1, "must deliver items too");
        assert!(
            delivered.windows(2).all(|w| w[1] == w[0] + 1),
            "underlying order must be preserved: {delivered:?}"
        );
    }

    /// Different rates must give **independent** patterns, not merely
    /// different ones — the case `seed_for` exists to cover.
    ///
    /// Asserting `pattern(0.23) != pattern(0.15)` would be too weak to be
    /// worth writing: two thresholds applied to one shared draw sequence
    /// also produce different output, while leaving the rarer stream's
    /// stalls strictly *nested* inside the commoner one's. Nesting is the
    /// failure this guards against, so the assertion is that each stream
    /// stalls somewhere the other does not.
    ///
    /// Verified failable: seeding both from a constant makes the second
    /// half fail, since nesting is exactly what a shared sequence yields.
    #[test]
    fn different_probabilities_are_independent_not_nested() {
        let a = pattern(0.23);
        let b = pattern(0.15);
        assert_ne!(a, b);
        let a_only = a.iter().zip(&b).any(|(x, y)| *x == -1 && *y != -1);
        let b_only = a.iter().zip(&b).any(|(x, y)| *x != -1 && *y == -1);
        assert!(
            a_only && b_only,
            "streams must stall independently, not nested: \
             a_only={a_only}, b_only={b_only}"
        );
    }

    /// `0.25` and `0.5` share their low 32 mantissa bits (both zero), so
    /// a seed taken from that half alone would collide. Pin the fold.
    #[test]
    fn probabilities_sharing_low_bits_do_not_collide() {
        assert_ne!(seed_for(0.25), seed_for(0.5));
    }

    /// The equal-probability case that `stalling_with_seed` exists for:
    /// same rate, same default seed, identical sequences — and distinct
    /// seeds break that tie.
    #[test]
    fn equal_probabilities_collide_unless_seeded_apart() {
        // lockstep-audit: intentional — demonstrating the collision is
        // the entire point of this test.
        let collect = |it: Box<dyn Iterator<Item = Option<i32>>>| {
            it.take(30).map(|x| x.unwrap_or(-1)).collect::<Vec<_>>()
        };
        let a = collect(Box::new(stalling(0..30, 0.23)));
        let b = collect(Box::new(stalling(0..30, 0.23)));
        assert_eq!(a, b, "same probability alone means lockstep");

        let c = collect(Box::new(stalling_with_seed(0..30, 0.23, 0xA1)));
        let d = collect(Box::new(stalling_with_seed(0..30, 0.23, 0xB2)));
        assert_ne!(c, d, "explicit seeds must decorrelate");
    }

    /// Probability >= 1.0 is rejected rather than silently hanging.
    #[test]
    #[should_panic(expected = "not supported")]
    fn probability_at_one_panics() {
        let _ = stalling(0..10, 1.0).take(1).count();
    }
}
