use super::*;

// A silent host is asked at each listing, or AGAIN after the last ask, never backing off; one
// being woken every SOON until it answers or WAKE_PROBES pass; one asleep only at ASLEEP_AGAIN.
// An answer lists at once.
#[test]
fn a_silent_host_is_asked_at_each_listing_and_every_few_seconds_while_it_wakes() {
    let health = Health::default();
    let start = Instant::now();
    assert!(health.stopped("mac", None, start));
    assert!(!health.stopped("mac", None, start + AGAIN));
    let asked = |at: Instant| {
        let mut asked = Vec::new();
        ask_again(&health, || Some(vec!["mac".into(), "e2".into()]), |host| { asked.push(host.to_string()); false }, || at);
        asked.sort();
        asked
    };
    assert_eq!(health.next("mac"), Some(start + AGAIN));
    for at in [start + AGAIN, start + 2 * AGAIN, start + 3 * AGAIN] {
        assert_eq!(asked(at), ["mac"]);
        assert_eq!(health.next("mac"), Some(at + AGAIN));
    }
    // A listing makes each one due now but an asleep machine.
    health.slept("e2", "tolkster-2", start);
    let listing = start + 3 * AGAIN + Duration::from_secs(1);
    health.listed(listing);
    assert_eq!((health.next("mac"), health.next("e2")), (Some(listing), Some(start + ASLEEP_AGAIN)));
    // A wake makes it due every SOON, until WAKE_PROBES have passed.
    health.waking("e2", listing);
    assert_eq!(health.next("e2"), Some(listing + SOON));
    assert_eq!(asked(listing + SOON), ["e2", "mac"]);
    assert_eq!((health.next("mac"), health.next("e2")), (Some(listing + SOON + AGAIN), Some(listing + 2 * SOON)));
    let woke = listing;
    let late = woke + WAKE_PROBES;
    asked(late);
    assert_eq!(health.next("e2"), Some(late + ASLEEP_AGAIN));
    // An answer runs what lists.
    let listed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = std::sync::Arc::clone(&listed);
    health.on_answer(move || {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    ask_again(&health, || Some(vec!["mac".into()]), |_| true, || late + ASLEEP_AGAIN);
    assert!(!health.silent("mac"));
    assert_eq!(listed.load(std::sync::atomic::Ordering::SeqCst), 1);
    // One heard while its wake runs is listed by the wake, once it has marked the machine awake.
    health.slept("e3", "tolkster-3", late);
    health.waking("e3", late);
    ask_again(&health, || Some(vec!["e3".into()]), |_| true, || late + SOON);
    assert!(!health.silent("e3"));
    assert_eq!(listed.load(std::sync::atomic::Ordering::SeqCst), 1);
    health.woken("e3", late + SOON);
    assert_eq!(listed.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn a_background_ask_puts_a_host_back_in_rotation_on_its_first_answer() {
    let health = Health::default();
    let start = Instant::now();
    health.stopped("mac", Some("Mac".into()), start);
    health.stopped("pi", None, start);
    ask_again(&health, || Some(vec!["mac".into(), "pi".into()]), |host| host == "mac", || start + AGAIN);
    assert!(!health.silent("mac") && health.next("mac").is_none());
    assert!(health.silent("pi"));
    // One no longer paired is forgotten, never asked.
    ask_again(&health, || Some(Vec::new()), |_| unreachable!(), || start + ASLEEP_AGAIN);
    assert!(!health.silent("pi"));
}

// A host unpaired is reported to the Durable Object, which releases its workers, only once the
// label its issues show is cleared: until then the listing keeps it among the paired.
#[test]
fn an_unpaired_host_is_kept_among_the_paired_until_its_label_is_cleared() {
    let health = Health::default();
    let start = Instant::now();
    let paired = |health: &Health, hosts: &[&str]| health.withholding(hosts.iter().map(|h| h.to_string()).collect(), start);
    health.stopped("mac", None, start);
    health.marked("mac", Mark::Offline);
    // Paired, it is no one's to hold back.
    assert_eq!(paired(&health, &["mac"]), ["mac"]);
    // Unpaired with its label shown: its clear is owed, and it stays in the list. `pi`, never labelled, is no one's to hold.
    assert_eq!(paired(&health, &["pi"]), ["pi", "mac"]);
    assert_eq!(health.owed(start), [("mac".to_string(), Mark::Clear)]);
    // A failed clear is owed again later, and the host stays.
    health.unmarked("mac", Mark::Clear, start);
    assert_eq!(paired(&health, &["pi"]), ["pi", "mac"]);
    // Once made, it is left out.
    health.marked("mac", Mark::Clear);
    assert_eq!(paired(&health, &["pi"]), ["pi"]);
}

// A host that reads its config is trusted for READS_FOR; one that refused is asked again sooner,
// so a rejoin is taken up.
#[test]
fn a_refusal_of_a_hosts_config_is_checked_again_sooner_than_a_reading() {
    let health = Health::default();
    let start = Instant::now();
    assert_eq!(health.reads("mac", start), None);
    health.read("mac", true, start);
    health.read("pc", false, start);
    assert_eq!((health.reads("mac", start + REFUSED_FOR), health.reads("pc", start + REFUSED_FOR)), (Some(true), None));
    assert_eq!(health.reads("mac", start + READS_FOR), None);
    assert!(health.refused("pc", start) && !health.refused("mac", start) && !health.refused("pc", start + REFUSED_FOR));
}

// A restart forgets what `slept` knew: a machine marked asleep is taken as silent, with no line and no
// label owed, and asked again at the longest wait, while a host that merely stopped answering is faulted.
#[test]
fn a_restart_takes_a_machine_marked_asleep_as_asleep_and_another_silent_host_as_faulted() {
    let health = Health::default();
    let start = Instant::now();
    health.marked_asleep([("tolkster-2", "e2")], start);
    assert!(!health.stopped("e2", None, start + Duration::from_secs(1)));
    assert!(health.owed(start + Duration::from_secs(1)).is_empty());
    assert_eq!(health.next("e2"), Some(start + ASLEEP_AGAIN));
    assert_eq!(health.silences(), [Silent { host: "e2".into(), name: Some("tolkster-2".into()), asleep: true, since: start, next: start + ASLEEP_AGAIN }]);
    assert!(health.stopped("mac", None, start));
    assert_eq!(health.owed(start), [("mac".to_string(), Mark::Offline)]);
    // The next listing's mark keeps a back-off already running.
    health.marked_asleep([("tolkster-2", "e2")], start + AGAIN);
    assert_eq!(health.next("e2"), Some(start + ASLEEP_AGAIN));
}
