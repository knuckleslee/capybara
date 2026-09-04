//! Finds the firmware's idle counter.
//!
//! Usage: cargo run --release --example inactivite_probe --
//!            <dump.bin> <hex key> <state.tamastate>
//!
//! The firmware falls asleep after a few minutes without a press. It therefore
//! holds somewhere in RAM a count that rises while nothing happens and falls
//! back as soon as a button is touched. Clearing it now and then is the only
//! clean way to prevent sleep: no forced scene, no wake by reset, nothing that
//! departs from what the firmware does naturally.
//!
//! The probe looks for it by its signature, which resembles nothing else:
//!
//! 1. three readings of RAM, spaced out, console idle
//! 2. a button press, then a fourth reading
//!
//! The counter wanted is the one that **rises strictly** across the first three
//! readings and **falls back** after the press. A game variable that rises does
//! not fall on a press; a variable that falls on a press was not rising on its
//! own. The two conditions together leave almost no ambiguity.

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
    let key = u32::from_str_radix(a.next().expect("cle hex").trim_start_matches("0x"), 16).unwrap();
    let etat_path = a.next().expect("etat.tamastate");

    let etat = Instantane::lire(std::path::Path::new(&etat_path)).expect("lecture de l'etat");
    let mut m = Machine::new();
    m.device_key = Some(key);
    m.load_firmware_file(&path).unwrap();
    m.restaurer(&etat);
    m.bus.mmio_trace.enabled = false;
    m.is_running = true;

    // A few seconds for the scene to settle after the restore.
    println!("  mise en route...");
    avancer(&mut m, 3.0);

    println!("  trois releves au repos, dix secondes d'ecart...");
    let r0 = releve(&m);
    avancer(&mut m, 10.0);
    let r1 = releve(&m);
    avancer(&mut m, 10.0);
    let r2 = releve(&m);

    println!("  appui sur A, puis un quatrieme releve...");
    m.appuyer(Machine::BOUTON_A);
    avancer(&mut m, 0.2);
    m.relacher(Machine::BOUTON_A);
    avancer(&mut m, 2.0);
    let r3 = releve(&m);

    let lire16 = |r: &[u8], o: usize| u16::from_le_bytes([r[o], r[o + 1]]) as u64;
    let lire32 = |r: &[u8], o: usize| {
        u32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]) as u64
    };

    for (largeur, lire) in [
        (2usize, &lire16 as &dyn Fn(&[u8], usize) -> u64),
        (4usize, &lire32 as &dyn Fn(&[u8], usize) -> u64),
    ] {
        println!("\n  candidats sur {} octets :", largeur);
        println!(
            "  {:<12} {:>10} {:>10} {:>10} {:>10}  {}",
            "adresse", "t=0", "t=10", "t=20", "apres appui", "pas par seconde"
        );
        let mut trouves = 0;
        let mut o = 0;
        while o + largeur <= r0.len() {
            let (a0, a1, a2, a3) = (lire(&r0, o), lire(&r1, o), lire(&r2, o), lire(&r3, o));
            // Rises strictly while idle, and falls clearly after the press.
            let monte = a1 > a0 && a2 > a1;
            let retombe = a3 * 2 < a2;
            // The two intervals must advance by a similar amount: a time
            // counter moves steadily, whereas a checksum or a pointer that
            // climbs in fits does not.
            let d1 = a1 - a0;
            let d2 = a2 - a1;
            let regulier = d1 > 0 && d2 > 0 && d1.max(d2) <= d1.min(d2) * 2;
            if monte && retombe && regulier {
                println!(
                    "  {:#010x}   {:>10} {:>10} {:>10} {:>10}  {:.2}",
                    BASE + o as u32,
                    a0,
                    a1,
                    a2,
                    a3,
                    (a2 - a0) as f64 / 20.0
                );
                trouves += 1;
            }
            o += largeur;
        }
        if trouves == 0 {
            println!("  (aucun)");
        }
    }

    println!(
        "\n  Le bon candidat avance de un a quelques dizaines par seconde et retombe\n  \
         a zero ou presque apres l'appui. S'il y en a plusieurs, relancer la sonde\n  \
         depuis un autre etat : seul le vrai compteur se comportera pareil partout."
    );
}
