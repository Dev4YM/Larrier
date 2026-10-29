//! Read-only smoke test over this machine's real agent sessions.
//! Imports everything into a throwaway store and prints counts and timing
//! (no message contents). `cargo run -p larrier-formats --example smoke`

use std::collections::BTreeMap;
use std::time::Instant;

use larrier_core::Store;

fn main() {
    let t0 = Instant::now();
    let agents = larrier_formats::importable_agents();
    let mut total = 0;
    for a in &agents {
        let t = Instant::now();
        let list = larrier_formats::discover(a);
        total += list.len();
        if !list.is_empty() {
            println!("discover {:<12} {:>5} sessions  {:>6} ms", a.id(), list.len(), t.elapsed().as_millis());
        }
    }
    let droid = larrier_formats::discover(&larrier_core::AgentKind::Droid);
    println!("discover {:<12} {:>5} sessions", "droid", droid.len());
    println!("discovered {total} in {} ms", t0.elapsed().as_millis());

    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut all = agents.clone();
    all.push(larrier_core::AgentKind::Droid);
    let t = Instant::now();
    let r = larrier_formats::sync(&store, &all);
    println!("sync #1: imported {} updated {} skipped {} errors {} in {} ms", r.imported, r.updated, r.skipped, r.errors.len(), t.elapsed().as_millis());
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for e in &r.errors {
        let k: String = e.split(':').skip(1).collect::<Vec<_>>().join(":").chars().take(120).collect();
        *kinds.entry(format!("{} {}", e.split(' ').next().unwrap_or(""), k)).or_default() += 1;
    }
    for (k, n) in kinds.iter().take(15) {
        println!("  err x{n}: {k}");
    }
    let list = store.list();
    let mut per: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    for s in &list {
        let e = per.entry(s.origin.id()).or_default();
        e.0 += 1;
        e.1 += s.message_count;
        e.2 += s.files_changed;
    }
    for (a, (n, m, f)) in &per {
        println!("  {a:<12} {n:>5} sessions {m:>7} messages {f:>6} files changed");
    }
    for s in list.iter().take(5) {
        println!("  recent: [{}] {} ({} msgs)", s.origin.id(), s.title, s.message_count);
    }
    let bytes: u64 = std::fs::read_dir(store.sessions_dir()).unwrap().flatten().map(|e| e.metadata().map(|m| m.len()).unwrap_or(0)).sum();
    println!("store size {:.1} MB", bytes as f64 / 1e6);
    let t = Instant::now();
    let r2 = larrier_formats::sync(&store, &all);
    println!("sync #2: imported {} updated {} skipped {} errors {} in {} ms", r2.imported, r2.updated, r2.skipped, r2.errors.len(), t.elapsed().as_millis());
    println!("total {} ms", t0.elapsed().as_millis());
}
