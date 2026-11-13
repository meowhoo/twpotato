//! Per-thread request counters, compiled only with `arch-prof`.
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub forward_ns: u64,
    pub head_ns: u64,
    pub calls: u64,
    pub l: usize,
    pub nsyl: usize,
}

thread_local! {
    static COUNTERS: Cell<Snapshot> = Cell::new(Snapshot::default());
}

pub fn add_forward(ns: u64, l: usize) {
    COUNTERS.with(|c| {
        let mut s = c.get();
        s.forward_ns += ns;
        s.calls += 1;
        s.l = l;
        c.set(s);
    });
}

pub fn add_head(ns: u64, nsyl: usize) {
    COUNTERS.with(|c| {
        let mut s = c.get();
        s.head_ns += ns;
        s.nsyl = nsyl;
        c.set(s);
    });
}

pub fn take() -> Snapshot {
    COUNTERS.with(|c| c.replace(Snapshot::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_resets_and_isolates_threads() {
        take();
        add_forward(11, 4);
        add_head(3, 2);
        std::thread::spawn(|| {
            assert_eq!(take(), Snapshot::default());
            add_forward(99, 9);
        }).join().unwrap();
        add_forward(17, 7);
        add_head(5, 3);
        assert_eq!(take(), Snapshot { forward_ns: 28, head_ns: 8, calls: 2, l: 7, nsyl: 3 });
        assert_eq!(take(), Snapshot::default());
    }
}
