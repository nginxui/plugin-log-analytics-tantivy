//! Hourly rollups for the dashboard.
//!
//! Every log group keeps, per UTC hour, the page views, the bytes, the status
//! classes, the views per minute, the set of distinct visitors and the counts
//! of browsers, systems, device types and paths. A dashboard request takes the
//! whole hours from here and reads the index only for the hours that a window
//! or a day boundary cuts, see [`crate::analytics`].
//!
//! The rollups are a cache of the index, never a second source of truth:
//!
//! * Indexing adds what it writes to a pending delta, under the same lock that
//!   protects the documents, and every commit applies the delta it made. So a
//!   rollup always equals the committed documents.
//! * A delete that cannot be subtracted (a file that was rewritten, a rebuild)
//!   drops the rollup of the group. It is computed again from the index, at the
//!   end of the round or by the first request that needs it.
//! * After a restart nothing is loaded. The first request scans the group once.
//! * A group whose rollup would take more than [`MAX_GROUP_BYTES`] is not
//!   kept, its dashboards read the index as before.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tantivy::collector::Collector;
use tantivy::columnar::{Column, StrColumn};
use tantivy::{DocId, Score, SegmentReader};

/// The fields the counters of a rollup follow: browser, system, device type, path.
pub const TERM_FIELDS: [&str; 4] = ["browser", "os", "device_type", "path"];

/// Memory a group may take before its rollup is dropped.
pub const MAX_GROUP_BYTES: usize = 64 << 20;

/// Stable 64 bit hash of a term. It has to stay the same between runs and
/// versions, so it is written out instead of taken from the standard library.
pub fn term_hash(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^ (h >> 31)
}

/// Hasher for keys that already are well mixed hashes.
#[derive(Default)]
pub struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(b);
        }
    }

    fn write_u64(&mut self, v: u64) {
        self.0 = v;
    }
}

pub type IdSet = HashSet<u64, BuildHasherDefault<IdHasher>>;
pub type IdMap<V> = HashMap<u64, V, BuildHasherDefault<IdHasher>>;

/// Start of the UTC hour of a timestamp.
pub fn hour_of(ts: i64) -> i64 {
    ts - ts.rem_euclid(3600)
}

/// Slot of a status code in [`HourAgg::status`]: the class 1 to 5, else 0.
pub fn status_class(status: u64) -> usize {
    if (100..600).contains(&status) {
        (status / 100) as usize
    } else {
        0
    }
}

/// The figures of one group in one UTC hour.
#[derive(Clone, Debug, PartialEq)]
pub struct HourAgg {
    pub pv: u64,
    pub bytes: u64,
    /// Views by status class: index 0 is anything outside 100 to 599.
    pub status: [u64; 6],
    /// Views per minute of the hour.
    pub minutes: [u32; 60],
    /// Hashes of the distinct visitor addresses, sorted.
    pub ips: Vec<u64>,
    /// Counts by term hash for [`TERM_FIELDS`], sorted by hash.
    pub terms: [Vec<(u64, u32)>; 4],
}

impl Default for HourAgg {
    fn default() -> Self {
        Self { pv: 0, bytes: 0, status: [0; 6], minutes: [0; 60], ips: Vec::new(), terms: Default::default() }
    }
}

fn merge_ips(a: &mut Vec<u64>, b: &[u64]) {
    if b.is_empty() {
        return;
    }
    if a.is_empty() {
        a.extend_from_slice(b);
        return;
    }
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => {
                out.push(a[i]);
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                out.push(b[j]);
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out.extend_from_slice(&a[i..]);
    out.extend_from_slice(&b[j..]);
    *a = out;
}

fn merge_counts(a: &mut Vec<(u64, u32)>, b: &[(u64, u32)]) {
    if b.is_empty() {
        return;
    }
    if a.is_empty() {
        a.extend_from_slice(b);
        return;
    }
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].0.cmp(&b[j].0) {
            std::cmp::Ordering::Less => {
                out.push(a[i]);
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                out.push(b[j]);
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                out.push((a[i].0, a[i].1 + b[j].1));
                i += 1;
                j += 1;
            }
        }
    }
    out.extend_from_slice(&a[i..]);
    out.extend_from_slice(&b[j..]);
    *a = out;
}

impl HourAgg {
    pub fn merge(&mut self, other: &HourAgg) {
        self.pv += other.pv;
        self.bytes += other.bytes;
        for (a, b) in self.status.iter_mut().zip(&other.status) {
            *a += *b;
        }
        for (a, b) in self.minutes.iter_mut().zip(&other.minutes) {
            *a += *b;
        }
        merge_ips(&mut self.ips, &other.ips);
        for (a, b) in self.terms.iter_mut().zip(&other.terms) {
            merge_counts(a, b);
        }
    }

    fn bytes_used(&self) -> usize {
        std::mem::size_of::<Self>() + self.ips.len() * 8 + self.terms.iter().map(|t| t.len() * 16).sum::<usize>()
    }
}

/// The hours of one group, and the text of every term hash they mention.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GroupRollup {
    pub hours: BTreeMap<i64, HourAgg>,
    pub names: [IdMap<Box<str>>; 4],
}

impl GroupRollup {
    pub fn merge(&mut self, other: GroupRollup) {
        for (hour, agg) in other.hours {
            match self.hours.get_mut(&hour) {
                Some(mine) => mine.merge(&agg),
                None => {
                    self.hours.insert(hour, agg);
                }
            }
        }
        for (mine, theirs) in self.names.iter_mut().zip(other.names) {
            for (hash, text) in theirs {
                mine.entry(hash).or_insert(text);
            }
        }
    }

    /// Bytes the rollup takes, about.
    pub fn bytes_used(&self) -> usize {
        self.hours.values().map(|h| h.bytes_used() + 48).sum::<usize>()
            + self.names.iter().map(|n| n.values().map(|t| t.len() + 56).sum::<usize>()).sum::<usize>()
    }

    /// Text of a term hash.
    pub fn name(&self, field: usize, hash: u64) -> Option<&str> {
        self.names[field].get(&hash).map(|t| &**t)
    }
}

/// One hour while it is collected.
#[derive(Default)]
struct HourBuilder {
    pv: u64,
    bytes: u64,
    status: [u64; 6],
    minutes: Vec<u32>,
    ips: Vec<u64>,
    counts: [IdMap<u32>; 4],
}

/// Collects the documents of a batch into a [`GroupRollup`].
#[derive(Default)]
pub struct RollupBuilder {
    hours: HashMap<i64, HourBuilder>,
    names: [IdMap<Box<str>>; 4],
    last: Option<i64>,
}

impl RollupBuilder {
    /// Adds one document. Empty terms are absent values and are not counted.
    pub fn add(&mut self, ts: i64, bytes: u64, status: u64, ip: &[u8], terms: [&str; 4]) {
        let hour = hour_of(ts);
        self.last = Some(hour);
        let h = self.hours.entry(hour).or_insert_with(|| HourBuilder { minutes: vec![0; 60], ..Default::default() });
        h.pv += 1;
        h.bytes += bytes;
        h.status[status_class(status)] += 1;
        h.minutes[(ts - hour) as usize / 60] += 1;
        h.ips.push(term_hash(ip));
        for (kind, term) in terms.into_iter().enumerate() {
            if term.is_empty() {
                continue;
            }
            let hash = term_hash(term.as_bytes());
            let count = h.counts[kind].entry(hash).or_insert(0);
            if *count == 0 {
                self.names[kind].entry(hash).or_insert_with(|| term.into());
            }
            *count += 1;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.hours.is_empty()
    }

    pub fn finish(self) -> GroupRollup {
        let hours = self
            .hours
            .into_iter()
            .map(|(hour, b)| {
                let mut agg = HourAgg { pv: b.pv, bytes: b.bytes, status: b.status, ..Default::default() };
                agg.minutes.copy_from_slice(&b.minutes);
                agg.ips = b.ips;
                agg.ips.sort_unstable();
                agg.ips.dedup();
                for (slot, counts) in agg.terms.iter_mut().zip(b.counts) {
                    *slot = counts.into_iter().collect();
                    slot.sort_unstable_by_key(|(hash, _)| *hash);
                }
                (hour, agg)
            })
            .collect();
        GroupRollup { hours, names: self.names }
    }
}

// ---------------------------------------------------------------- the store

/// What the store holds for a group.
#[derive(Clone)]
pub enum Slot {
    Ready(Arc<GroupRollup>),
    /// The rollup would be too large, the group is served from the index.
    TooLarge,
}

/// Changes made since the last commit.
#[derive(Default)]
pub struct Delta {
    groups: HashMap<String, GroupRollup>,
    invalid: HashSet<String>,
    all: bool,
}

struct Inner {
    /// Counts the applied commits and invalidations.
    version: u64,
    slots: HashMap<String, Slot>,
}

/// The rollups of all groups and the delta of the commit in the making.
pub struct Rollups {
    inner: Mutex<Inner>,
    pending: Mutex<Delta>,
    max_bytes: AtomicUsize,
}

impl Default for Rollups {
    fn default() -> Self {
        Self::new(MAX_GROUP_BYTES)
    }
}

impl Rollups {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(Inner { version: 0, slots: HashMap::new() }),
            pending: Mutex::new(Delta::default()),
            max_bytes: AtomicUsize::new(max_bytes),
        }
    }

    /// Changes the size a group may take, for tests.
    pub fn set_max_bytes(&self, max_bytes: usize) {
        self.max_bytes.store(max_bytes, Ordering::Relaxed);
    }

    /// Adds the documents of a batch. The caller holds the writer lock that
    /// keeps a commit from running, so the delta and the documents agree.
    pub fn add(&self, group: &str, batch: GroupRollup) {
        let mut p = self.pending.lock().expect("pending lock");
        p.groups.entry(group.to_owned()).or_default().merge(batch);
    }

    /// Drops the rollup of a group at the next commit, after documents were
    /// deleted that cannot be subtracted.
    pub fn invalidate(&self, group: &str) {
        let mut p = self.pending.lock().expect("pending lock");
        p.groups.remove(group);
        p.invalid.insert(group.to_owned());
    }

    /// Drops every rollup at the next commit.
    pub fn invalidate_all(&self) {
        let mut p = self.pending.lock().expect("pending lock");
        p.groups.clear();
        p.invalid.clear();
        p.all = true;
    }

    /// Forgets everything now, for an index that was replaced.
    pub fn clear(&self) {
        let mut inner = self.inner.lock().expect("rollups lock");
        inner.slots.clear();
        inner.version += 1;
        *self.pending.lock().expect("pending lock") = Delta::default();
    }

    /// Takes the delta of the commit that is about to run.
    pub fn take_pending(&self) -> Delta {
        std::mem::take(&mut *self.pending.lock().expect("pending lock"))
    }

    /// Puts a delta back after its commit failed.
    pub fn restore(&self, delta: Delta) {
        let mut p = self.pending.lock().expect("pending lock");
        for (group, batch) in delta.groups {
            p.groups.entry(group).or_default().merge(batch);
        }
        p.invalid.extend(delta.invalid);
        p.all |= delta.all;
    }

    /// Applies the delta of a commit. `after` runs in the same critical
    /// section and makes the commit visible to readers, so a reader that takes
    /// [`Rollups::snapshot`] sees the rollups and the index of one commit.
    pub fn apply(&self, delta: Delta, after: impl FnOnce()) {
        let mut inner = self.inner.lock().expect("rollups lock");
        if delta.all {
            inner.slots.clear();
        }
        for group in &delta.invalid {
            inner.slots.remove(group);
        }
        let limit = self.max_bytes.load(Ordering::Relaxed);
        for (group, batch) in delta.groups {
            let too_large = match inner.slots.get_mut(&group) {
                Some(Slot::Ready(rollup)) => {
                    let r = Arc::make_mut(rollup);
                    r.merge(batch);
                    r.bytes_used() > limit
                }
                // Absent groups are computed from the index later
                _ => false,
            };
            if too_large {
                inner.slots.insert(group, Slot::TooLarge);
            }
        }
        after();
        inner.version += 1;
    }

    /// Starts the rollup of a group that has no documents yet, so the
    /// documents of its first import are counted as they come.
    pub fn begin_group(&self, group: &str, documents: impl FnOnce() -> u64) {
        let mut inner = self.inner.lock().expect("rollups lock");
        if !inner.slots.contains_key(group) && documents() == 0 {
            inner.slots.insert(group.to_owned(), Slot::Ready(Arc::new(GroupRollup::default())));
        }
    }

    pub fn get(&self, group: &str) -> Option<Slot> {
        self.inner.lock().expect("rollups lock").slots.get(group).cloned()
    }

    /// The names of the groups that have a slot.
    pub fn groups(&self) -> Vec<String> {
        self.inner.lock().expect("rollups lock").slots.keys().cloned().collect()
    }

    /// Runs `f` while no commit is applied and returns the version with its
    /// result. Take the searcher in `f`.
    pub fn snapshot<T>(&self, f: impl FnOnce() -> T) -> (u64, T) {
        let inner = self.inner.lock().expect("rollups lock");
        (inner.version, f())
    }

    /// Stores a rollup that was computed from the snapshot of `version`. It is
    /// dropped when a commit came in since.
    pub fn install(&self, version: u64, group: &str, rollup: GroupRollup) -> Slot {
        let slot = if rollup.bytes_used() > self.max_bytes.load(Ordering::Relaxed) {
            Slot::TooLarge
        } else {
            Slot::Ready(Arc::new(rollup))
        };
        let mut inner = self.inner.lock().expect("rollups lock");
        if inner.version == version {
            inner.slots.insert(group.to_owned(), slot.clone());
        }
        slot
    }
}

// ------------------------------------------------- computing from the index

/// Plain bitset over term ordinals.
#[derive(Default)]
struct Ords(Vec<u64>);

impl Ords {
    fn with_len(n: usize) -> Self {
        Ords(vec![0; n.div_ceil(64)])
    }

    fn set(&mut self, i: u64) {
        self.0[(i >> 6) as usize] |= 1 << (i & 63);
    }

    fn list(&self) -> Vec<u64> {
        let mut out = Vec::new();
        for (w, &bits) in self.0.iter().enumerate() {
            let mut b = bits;
            while b != 0 {
                out.push((w as u64) * 64 + u64::from(b.trailing_zeros()));
                b &= b - 1;
            }
        }
        out
    }
}

/// Texts of the ordinals of a column, in one pass over its dictionary.
fn ord_texts(col: &StrColumn, ords: &[u64]) -> HashMap<u64, Box<str>> {
    let mut out = HashMap::with_capacity(ords.len());
    let mut it = ords.iter();
    let _ = col.dictionary().sorted_ords_to_term_cb(ords.iter().copied(), |term| {
        if let Some(ord) = it.next() {
            out.insert(*ord, String::from_utf8_lossy(term).into_owned().into_boxed_str());
        }
        Ok(())
    });
    out
}

/// One hour of one segment, with ordinals instead of texts.
struct SegHour {
    pv: u64,
    bytes: u64,
    status: [u64; 6],
    minutes: [u32; 60],
    ips: Vec<u32>,
    counts: [HashMap<u32, u32>; 4],
}

impl SegHour {
    fn new() -> Self {
        Self { pv: 0, bytes: 0, status: [0; 6], minutes: [0; 60], ips: Vec::new(), counts: Default::default() }
    }
}

/// Builds the rollup of the documents a query matches.
pub struct RollupCollector;

pub struct RollupSegment {
    ts: Option<Column<i64>>,
    bytes: Option<Column<u64>>,
    status: Option<Column<u64>>,
    ip: Option<StrColumn>,
    terms: [Option<StrColumn>; 4],
    hours: Vec<(i64, SegHour)>,
    /// Where an hour is in `hours`, and the hour that was used last.
    slot: HashMap<i64, usize>,
    last: usize,
}

impl Collector for RollupCollector {
    type Fruit = GroupRollup;
    type Child = RollupSegment;

    fn for_segment(&self, _id: u32, reader: &SegmentReader) -> tantivy::Result<RollupSegment> {
        let ff = reader.fast_fields();
        let mut terms: [Option<StrColumn>; 4] = Default::default();
        for (slot, name) in terms.iter_mut().zip(TERM_FIELDS) {
            *slot = ff.str(name)?;
        }
        Ok(RollupSegment {
            ts: ff.column_opt::<i64>("ts")?,
            bytes: ff.column_opt::<u64>("bytes_sent")?,
            status: ff.column_opt::<u64>("status")?,
            ip: ff.str("ip")?,
            terms,
            hours: Vec::new(),
            slot: HashMap::new(),
            last: 0,
        })
    }

    fn requires_scoring(&self) -> bool {
        false
    }

    fn merge_fruits(&self, fruits: Vec<GroupRollup>) -> tantivy::Result<GroupRollup> {
        let mut out = GroupRollup::default();
        for f in fruits {
            out.merge(f);
        }
        Ok(out)
    }
}

impl tantivy::collector::SegmentCollector for RollupSegment {
    type Fruit = GroupRollup;

    fn collect(&mut self, doc: DocId, _score: Score) {
        let Some(ts) = self.ts.as_ref().and_then(|c| c.first(doc)) else { return };
        let hour = hour_of(ts);
        // Logs are mostly in time order, so the last hour is usually the one
        if self.hours.get(self.last).is_none_or(|(h, _)| *h != hour) {
            self.last = match self.slot.get(&hour) {
                Some(&i) => i,
                None => {
                    self.hours.push((hour, SegHour::new()));
                    self.slot.insert(hour, self.hours.len() - 1);
                    self.hours.len() - 1
                }
            };
        }
        let h = &mut self.hours[self.last].1;
        h.pv += 1;
        h.bytes += self.bytes.as_ref().and_then(|c| c.first(doc)).unwrap_or(0);
        h.status[status_class(self.status.as_ref().and_then(|c| c.first(doc)).unwrap_or(0))] += 1;
        h.minutes[(ts - hour) as usize / 60] += 1;
        if let Some(o) = self.ip.as_ref().and_then(|c| c.ords().first(doc)) {
            h.ips.push(o as u32);
        }
        for (kind, col) in self.terms.iter().enumerate() {
            if let Some(o) = col.as_ref().and_then(|c| c.ords().first(doc)) {
                *h.counts[kind].entry(o as u32).or_insert(0) += 1;
            }
        }
    }

    fn harvest(self) -> GroupRollup {
        let mut out = GroupRollup::default();
        // One dictionary pass per column for every ordinal of the segment
        let mut ip_seen = Ords::with_len(self.ip.as_ref().map_or(0, |c| c.num_terms()));
        let mut seen: [Ords; 4] =
            std::array::from_fn(|k| Ords::with_len(self.terms[k].as_ref().map_or(0, |c| c.num_terms())));
        for (_, h) in &self.hours {
            for &o in &h.ips {
                ip_seen.set(u64::from(o));
            }
            for (kind, counts) in h.counts.iter().enumerate() {
                for &o in counts.keys() {
                    seen[kind].set(u64::from(o));
                }
            }
        }
        let ip_hashes: HashMap<u64, u64> = match &self.ip {
            Some(col) => {
                ord_texts(col, &ip_seen.list()).into_iter().map(|(o, t)| (o, term_hash(t.as_bytes()))).collect()
            }
            None => HashMap::new(),
        };
        let texts: [HashMap<u64, Box<str>>; 4] = std::array::from_fn(|k| match &self.terms[k] {
            Some(col) => ord_texts(col, &seen[k].list()),
            None => HashMap::new(),
        });
        for (kind, map) in texts.iter().enumerate() {
            for text in map.values() {
                out.names[kind].insert(term_hash(text.as_bytes()), text.clone());
            }
        }
        for (hour, h) in self.hours {
            let mut agg =
                HourAgg { pv: h.pv, bytes: h.bytes, status: h.status, minutes: h.minutes, ..Default::default() };
            agg.ips = h.ips.iter().filter_map(|o| ip_hashes.get(&u64::from(*o)).copied()).collect();
            agg.ips.sort_unstable();
            agg.ips.dedup();
            for (kind, counts) in h.counts.iter().enumerate() {
                let mut merged: IdMap<u32> = IdMap::default();
                for (o, c) in counts {
                    if let Some(text) = texts[kind].get(&u64::from(*o)) {
                        *merged.entry(term_hash(text.as_bytes())).or_insert(0) += *c;
                    }
                }
                agg.terms[kind] = merged.into_iter().collect();
                agg.terms[kind].sort_unstable_by_key(|(hash, _)| *hash);
            }
            out.hours.insert(hour, agg);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builder(docs: &[(i64, &str, &str)]) -> GroupRollup {
        let mut b = RollupBuilder::default();
        for (ts, ip, path) in docs {
            b.add(*ts, 10, 200 + (*ts % 3) as u64 * 100, ip.as_bytes(), ["Chrome", "", "", path]);
        }
        b.finish()
    }

    #[test]
    fn hashes_are_stable() {
        // Pinned values: the rollups of different versions have to agree
        assert_eq!(term_hash(b""), 0xf52a15e9a9b5e89b);
        assert_ne!(term_hash(b"1.2.3.4"), term_hash(b"1.2.3.5"));
        assert_eq!(term_hash(b"1.2.3.4"), term_hash(b"1.2.3.4"));
    }

    #[test]
    fn a_batch_counts_views_visitors_minutes_and_terms() {
        let r = builder(&[(3600 + 5, "a", "/x"), (3600 + 65, "a", "/y"), (3600 + 70, "b", "/x"), (7200, "a", "/x")]);
        assert_eq!(r.hours.len(), 2);
        let first = &r.hours[&3600];
        assert_eq!((first.pv, first.bytes), (3, 30));
        assert_eq!(first.ips.len(), 2);
        assert_eq!((first.minutes[0], first.minutes[1]), (1, 2));
        assert_eq!(first.terms[0].len(), 1);
        assert_eq!(first.terms[0][0].1, 3);
        assert!(first.terms[1].is_empty());
        assert_eq!(first.terms[3].iter().map(|t| t.1).sum::<u32>(), 3);
        assert_eq!(r.name(3, term_hash(b"/x")), Some("/x"));
        assert_eq!(first.status.iter().sum::<u64>(), 3);
    }

    #[test]
    fn merging_batches_equals_one_batch() {
        let docs: Vec<(i64, &str, &str)> = (0..200)
            .map(|i| {
                (
                    3600 * (i % 3) + i * 7,
                    ["a", "b", "c", "d", "e"][(i % 5) as usize],
                    ["/x", "/y", "/z"][(i % 3) as usize],
                )
            })
            .collect();
        let whole = builder(&docs);
        let mut merged = builder(&docs[..70]);
        merged.merge(builder(&docs[70..130]));
        merged.merge(builder(&docs[130..]));
        assert_eq!(merged, whole);
    }

    #[test]
    fn apply_merges_into_ready_groups_and_drops_the_rest() {
        let rollups = Rollups::default();
        rollups.begin_group("a", || 0);
        rollups.begin_group("b", || 5);
        assert!(matches!(rollups.get("a"), Some(Slot::Ready(_))));
        assert!(rollups.get("b").is_none());

        rollups.add("a", builder(&[(10, "x", "/p")]));
        rollups.add("b", builder(&[(10, "x", "/p")]));
        let mut visible = false;
        rollups.apply(rollups.take_pending(), || visible = true);
        assert!(visible);
        let Some(Slot::Ready(a)) = rollups.get("a") else { panic!("ready") };
        assert_eq!(a.hours[&0].pv, 1);
        assert!(rollups.get("b").is_none());

        // An invalidation wins over the adds that follow it
        rollups.invalidate("a");
        rollups.add("a", builder(&[(20, "y", "/q")]));
        rollups.apply(rollups.take_pending(), || {});
        assert!(rollups.get("a").is_none());
    }

    #[test]
    fn a_snapshot_is_dropped_when_a_commit_came_in() {
        let rollups = Rollups::default();
        let (version, ()) = rollups.snapshot(|| ());
        rollups.apply(Delta::default(), || {});
        rollups.install(version, "g", GroupRollup::default());
        assert!(rollups.get("g").is_none());
        let (version, ()) = rollups.snapshot(|| ());
        rollups.install(version, "g", GroupRollup::default());
        assert!(rollups.get("g").is_some());
    }

    #[test]
    fn an_oversized_rollup_is_not_kept() {
        let rollups = Rollups::new(2000);
        rollups.begin_group("a", || 0);
        let docs: Vec<(i64, String, &str)> = (0..500).map(|i| (i, format!("ip{i}"), "/p")).collect();
        let mut b = RollupBuilder::default();
        for (ts, ip, path) in &docs {
            b.add(*ts, 1, 200, ip.as_bytes(), ["", "", "", path]);
        }
        rollups.add("a", b.finish());
        rollups.apply(rollups.take_pending(), || {});
        assert!(matches!(rollups.get("a"), Some(Slot::TooLarge)));
        let slot = rollups.install(
            rollups.snapshot(|| ()).0,
            "c",
            builder(&docs.iter().map(|(t, i, p)| (*t, i.as_str(), *p)).collect::<Vec<_>>()),
        );
        assert!(matches!(slot, Slot::TooLarge));
    }

    #[test]
    fn a_failed_commit_puts_its_delta_back() {
        let rollups = Rollups::default();
        rollups.begin_group("a", || 0);
        rollups.add("a", builder(&[(10, "x", "/p")]));
        let delta = rollups.take_pending();
        rollups.add("a", builder(&[(20, "y", "/p")]));
        rollups.restore(delta);
        rollups.apply(rollups.take_pending(), || {});
        let Some(Slot::Ready(a)) = rollups.get("a") else { panic!("ready") };
        assert_eq!(a.hours[&0].pv, 2);
    }
}
