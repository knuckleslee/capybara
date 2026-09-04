//! Measures what the fast-forward gains.
//!
//! Usage: cargo run --release --example repos_probe --
//!            <dump.bin> <hex key> <state.tamastate> [real seconds]
//!
//! The probe runs the same wall-clock duration twice, once with idle detection
//! and once without, and compares. It is the one measurement that says whether
//! the firmware spends its time waiting: if the skipped share is near zero, it
//! really is computing and the speed must be looked for elsewhere.

use capybara::emulator::etat::Instantane;
use capybara::emulator::peripherals::snsys::CYCLES_PAR_SECONDE;
use capybara::emulator::{Machine, StepResult};

fn main() {
    let mut a = std::env::args().skip(1);
    let path = a.next().expect("dump.bin");
    let key = u32::from_str_radix(a.next().expect("cle hex").trim_start_matches("0x"), 16).unwrap();
    let etat_path = a.next().expect("etat.tamastate");
    let reelles: f64 = a.next().and_then(|v| v.parse().ok()).unwrap_or(5.0);

    let etat = Instantane::lire(std::path::Path::new(&etat_path)).expect("lecture de l'etat");

    let mut resultats = Vec::new();
    for repos in [false, true] {
        let mut m = Machine::new();
        m.device_key = Some(key);
        m.load_firmware_file(&path).unwrap();
        m.restaurer(&etat);
        m.bus.mmio_trace.enabled = false;
        m.cpu.repos_actif = repos;
        m.is_running = true;

        let cycles_debut = m.cpu.cycles;
        let debut = std::time::Instant::now();
        while debut.elapsed().as_secs_f64() < reelles {
            if !matches!(m.run_frame(), StepResult::Ok(_)) {
                break;
            }
        }
        let ecoule = debut.elapsed().as_secs_f64();
        let cycles = (m.cpu.cycles - cycles_debut) as f64;
        let fois = cycles / ecoule / CYCLES_PAR_SECONDE as f64;
        let part = if cycles > 0.0 {
            m.cpu.cycles_sautes as f64 * 100.0 / cycles
        } else {
            0.0
        };
        println!(
            "  repos {:<7} : {:>8.2} millions de cycles par seconde, {:.2} fois le temps reel\
             \n                  {:.1}% du temps de console saute en {} avances",
            if repos { "actif" } else { "coupe" },
            cycles / ecoule / 1e6,
            fois,
            part,
            m.cpu.sauts
        );
        resultats.push(fois);
    }

    if resultats.len() == 2 && resultats[0] > 0.0 {
        println!(
            "\n  gain : x{:.2}",
            resultats[1] / resultats[0]
        );
        if resultats[1] / resultats[0] < 1.1 {
            println!(
                "  Le firmware ne s'arrete pratiquement jamais : la detection de repos\n  \
                 n'est pas la bonne piste ici. Regarder le decodage et la table de saut."
            );
        }
    }
}
