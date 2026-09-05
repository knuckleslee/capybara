//! Finds what the firmware consults to decide it has been idle.
//!
//! Usage: cargo run --release --example inactivite_probe --
//!            <dump.bin> <hex key> [state.tamastate]
//!
//! On the water edition it is a count: the half-word at `0x18001BFE` rises about
//! twenty times a second and is compared against the threshold at `0x18001C02`
//! at `0x00003238`. When the count wins, the firmware sets a bit that sends the
//! scene machine to the shutdown scene.
//!
//! That address was the first this probe reported, and it was right. It was
//! dismissed because clearing it did not appear to help — the fault was
//! elsewhere, in a one-sided comparison that stopped the clearing from running
//! after the first shutdown. A matching signature is not proof, but neither is
//! one failed attempt a refutation.
//!
//! Idleness is sometimes written the other way round: rather than counting up,
//! the firmware records the second of the last press and compares `now - then`
//! against a threshold. Such a word does not move while idle and jumps to the
//! current time on a press, so a search for a rising value could never report
//! it. Both shapes are searched here, the timestamp first because its signature
//! is the tighter of the two — for a word to become exactly the seconds counter
//! at the moment of a press is close to impossible by chance.

use capybara::emulator::etat::Instantane;
use capybara::emulator::peripherals::snsys::CYCLES_PAR_SECONDE;
use capybara::emulator::{Machine, StepResult};

const BASE: u32 = 0x1800_0000;

/// Advances the console by a number of seconds of its own time.
fn avancer(m: &mut Machine, secondes: f64) {
    let but = m.cpu.cycles + (CYCLES_PAR_SECONDE as f64 * secondes) as u64;
    while m.cpu.cycles < but {
        if !matches!(m.run_frame(), StepResult::Ok(_)) {
            break;
        }
    }
}

fn releve(m: &Machine) -> Vec<u8> {
    m.bus.sram.data.clone()
}

fn main() {
    let mut a = std::env::args().skip(1);
    let path = a.next().expect("dump.bin");
    let key = u32::from_str_radix(a.next().expect("hex key").trim_start_matches("0x"), 16).unwrap();
    let etat_path = a.next();

    let mut m = Machine::new();
    m.device_key = Some(key);
    m.load_firmware_file(&path).unwrap();
    if let Some(p) = etat_path.as_deref() {
        if !p.is_empty() && p != "-" {
            let etat = Instantane::lire(std::path::Path::new(p)).expect("reading the state");
            m.restaurer(&etat);
            println!("  starting from {p}");
        }
    }
    m.bus.mmio_trace.enabled = false;
    m.is_running = true;

    // A few seconds for the scene to settle after the restore.
    println!("  settling...");
    avancer(&mut m, 3.0);

    println!("  three readings at rest, ten seconds apart...");
    let r0 = releve(&m);
    let s0 = m.periph.snsys.secondes;
    avancer(&mut m, 10.0);
    let r1 = releve(&m);
    avancer(&mut m, 10.0);
    let r2 = releve(&m);
    let s2 = m.periph.snsys.secondes;

    println!("  pressing A, then a fourth reading...");
    m.appuyer(Machine::BOUTON_A);
    avancer(&mut m, 0.2);
    m.relacher(Machine::BOUTON_A);
    avancer(&mut m, 2.0);
    let r3 = releve(&m);
    let s3 = m.periph.snsys.secondes;

    println!("\n  seconds counter: {s0} at rest, {s2} before the press, {s3} after\n");

    let lire16 = |r: &[u8], o: usize| u16::from_le_bytes([r[o], r[o + 1]]) as u64;
    let lire32 = |r: &[u8], o: usize| {
        u32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]) as u64
    };

    // ---- the timestamp shape ------------------------------------------------
    //
    // Still while idle, and equal to the seconds counter after the press. A
    // couple of seconds of slack because the reading is taken two seconds after
    // the press and the firmware may stamp it a moment later.
    for (largeur, lire) in [
        (2usize, &lire16 as &dyn Fn(&[u8], usize) -> u64),
        (4usize, &lire32 as &dyn Fn(&[u8], usize) -> u64),
    ] {
        println!("  timestamps, {largeur} bytes:");
        println!(
            "  {:<12} {:>12} {:>12} {:>14}  {}",
            "address", "at rest", "before", "after press", "gap to the clock"
        );
        let mut trouves = 0;
        let mut o = 0;
        while o + largeur <= r0.len() {
            let (a0, a1, a2, a3) = (lire(&r0, o), lire(&r1, o), lire(&r2, o), lire(&r3, o));
            let immobile = a0 == a1 && a1 == a2;
            let bouge = a3 != a2;
            let ecart = (a3 as i64 - s3 as i64).abs();
            if immobile && bouge && ecart <= 3 {
                println!(
                    "  {:#010x}   {:>12} {:>12} {:>14}  {:+}",
                    BASE + o as u32,
                    a0,
                    a2,
                    a3,
                    a3 as i64 - s3 as i64
                );
                trouves += 1;
            }
            o += largeur;
        }
        if trouves == 0 {
            println!("  (none)");
        }
        println!();
    }

    // ---- the counter shape, kept for completeness ---------------------------
    println!("  counters (rising while idle, cleared on the press), 2 bytes:");
    println!(
        "  {:<12} {:>10} {:>10} {:>10} {:>14}  {}",
        "address", "t=0", "t=10", "t=20", "after press", "per second"
    );
    let mut trouves = 0;
    let mut o = 0;
    while o + 2 <= r0.len() {
        let (a0, a1, a2, a3) = (lire16(&r0, o), lire16(&r1, o), lire16(&r2, o), lire16(&r3, o));
        let monte = a1 > a0 && a2 > a1;
        let retombe = a3 * 2 < a2;
        let d1 = a1 - a0;
        let d2 = a2 - a1;
        // A time counter advances steadily; a checksum or a pointer climbing in
        // fits does not. Anything faster than a few tens per second is not a
        // shutdown timer, whose threshold is measured in minutes.
        let regulier = d1 > 0 && d2 > 0 && d1.max(d2) <= d1.min(d2) * 2 && (a2 - a0) <= 2000;
        if monte && retombe && regulier {
            println!(
                "  {:#010x}   {:>10} {:>10} {:>10} {:>14}  {:.2}",
                BASE + o as u32,
                a0,
                a1,
                a2,
                a3,
                (a2 - a0) as f64 / 20.0
            );
            trouves += 1;
        }
        o += 2;
    }
    if trouves == 0 {
        println!("  (none)");
    }

    println!(
        "\n  A timestamp is the stronger find: give its address to\n  \
         CAPYBARA_HORODATAGE_ACTIVITE and the emulator will keep it at the\n  \
         current second, so the firmware never sees any time pass since the last\n  \
         press. Several addresses may be given, separated by commas."
    );
}
