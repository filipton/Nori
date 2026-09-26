//! A short list that changes while it is on screen (the equalizer's devices): which rows are new, which
//! stay and which are leaving, and where a leaving row stands while it goes, so that a row folds away in
//! its own place instead of the rows under it jumping up over it. The platform animates; this keeps the
//! list.

/// One row of the list as it is drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row<K> {
    pub key: K,
    /// It is on screen now (all or part of the way).
    pub shown: bool,
    /// It is meant to be: false while it leaves.
    pub wanted: bool,
}

/// The rows to draw after the list became `keys`, from the rows drawn before (`before`, none the first
/// time). A row still in the list stays where the list puts it; a new one comes in (straight away the first
/// time, since what is there when the list first draws is simply there); one gone from the list is kept at
/// the place it had, as far as the new list reaches, going, until it has gone (not `shown`) - then it is
/// dropped.
///
/// Twin of `AnimatedRows` (app/.../ui/DevicesSection.kt), which Android keeps: there `shown` is the row's
/// `MutableTransitionState.currentState` and `wanted` its `targetState`.
pub fn merge_rows<K: Clone + PartialEq>(before: Option<&[Row<K>]>, keys: &[K]) -> Vec<Row<K>> {
    let old = before.unwrap_or(&[]);
    let mut next: Vec<Row<K>> = keys
        .iter()
        .map(|k| match old.iter().rev().find(|r| r.key == *k) {
            Some(r) => Row { key: k.clone(), shown: r.shown, wanted: true },
            None => Row { key: k.clone(), shown: before.is_none(), wanted: true },
        })
        .collect();
    for (i, r) in old.iter().enumerate() {
        if !keys.contains(&r.key) && (r.shown || r.wanted) {
            let at = i.min(next.len());
            next.insert(at, Row { key: r.key.clone(), shown: r.shown, wanted: false });
        }
    }
    next
}
