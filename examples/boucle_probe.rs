//! Where the cycles go: the hottest short loops, disassembled, with what
//! changes from one iteration to the next.
//!
//! Usage: cargo run --release --example boucle_probe --
//!            <dump.bin> <hex key> <state.tamastate> [real seconds]
//!
//! `repos_probe` says whether the fast-forward fires; this one says why it does
//! not. For each short loop head (a backward branch of less than sixty-four
//! bytes) it counts the iterations and the cycles spent there, then disassembles
//! the five hottest and shows, iteration by iteration, which registers move and
//! by how much. That is how one decides which recognition criterion to add.

use std::collections::HashMap;

use capybara::emulator::etat::Instantane;
use capybara::emulator::peripherals::snsys::CYCLES_PAR_SECONDE;
use capybara::emulator::{Disassembler, Machine, StepResult};

const PORTEE: u32 = 64;

#[derive(Default, Clone, Copy)]
struct Compte {
    tours: u64,
    cycles: u64,
}

fn main() {
    let mut a = std::env::args().skip(1);
    let path = a.next().expect("dump.bin");
    let key = u32::from_str_radix(a.next().expect("cle hex").trim_start_matches("0x"), 16).unwrap();
    let etat_path = a.next().expect("etat.tamastate");
    let reelles: f64 = a.next().and_then(|v| v.parse().ok()).unwrap_or(5.0);

    let etat = Instantane::lire(std::path::Path::new(&etat_path)).expect("lecture de l'etat");
    let mut m = Machine::new();
    m.device_key = Some(key);
    m.load_firmware_file(&path).unwrap();
    m.restaurer(&etat);
    m.bus.mmio_trace.enabled = false;
    // Recognition is switched off: we want the firmware as it really is.
    m.cpu.repos_actif = false;
    m.is_running = true;

    // ---- 1. Histogram of loop heads ---------------------------------------
    let mut comptes: HashMap<u32, Compte> = HashMap::new();
    let mut dernier_arriere = m.cpu.cycles;
    let cycles_debut = m.cpu.cycles;
    let debut = std::time::Instant::now();
    while debut.elapsed().as_secs_f64() < reelles {
        for _ in 0..20_000 {
            let avant = m.cpu.regs.pc;
            if !matches!(m.step(), StepResult::Ok(_)) {
                break;
            }
            let apres = m.cpu.regs.pc;
            if apres < avant && avant - apres <= PORTEE {
                let c = comptes.entry(apres).or_default();
                c.tours += 1;
                c.cycles += m.cpu.cycles - dernier_arriere;
                dernier_arriere = m.cpu.cycles;
            }
        }
    }
    let total = (m.cpu.cycles - cycles_debut) as f64;
    let ecoule = debut.elapsed().as_secs_f64();
    println!(
        "  {:.2} millions de cycles par seconde, {:.2} fois le temps reel, reconnaissance coupee\n",
        total / ecoule / 1e6,
        total / ecoule / CYCLES_PAR_SECONDE as f64
    );

    let mut tri: Vec<(u32, Compte)> = comptes.into_iter().collect();
    tri.sort_by(|x, y| y.1.cycles.cmp(&x.1.cycles));
    println!("  tetes de boucle courte, par cycles passes :");
    println!("  {:<12} {:>12} {:>14} {:>8} {:>10}", "tete", "tours", "cycles", "part", "cyc/tour");
    for (tete, c) in tri.iter().take(12) {
        println!(
            "  {:#010x}   {:>12} {:>14} {:>7.1}% {:>10.1}",
            tete,
            c.tours,
            c.cycles,
            c.cycles as f64 * 100.0 / total,
            c.cycles as f64 / c.tours.max(1) as f64
        );
    }

    // ---- 2. Disassembly of the five hottest --------------------------------
    let chaudes: Vec<u32> = tri.iter().take(5).map(|(t, _)| *t).collect();
    for tete in &chaudes {
        println!("\n  ---- boucle {:#010x} ----", tete);
        let mut adr = tete.saturating_sub(8) & !1;
        let fin = tete + PORTEE + 8;
        while adr < fin {
            let w1 = m.bus.read_u16(adr, &mut m.periph, &m.cpu.nvic);
            let w2 = m.bus.read_u16(adr + 2, &mut m.periph, &m.cpu.nvic);
            let d = Disassembler::disassemble(adr, &[w1, w2]);
            let marque = if adr == *tete { ">" } else { " " };
            println!("  {} {:#010x}  {:<8} {}", marque, adr, d.mnemonic, d.operands);
            adr += if d.is_32bit { 4 } else { 2 };
        }
    }

    // ---- 3. What changes between iterations, for the three hottest ---------
    println!("\n  registres qui bougent entre deux passages en tete (8 premiers passages) :");
    for tete in chaudes.iter().take(3) {
        let mut vus = 0usize;
        let mut precedent: Option<[u32; 17]> = None;
        let mut precedent_ecrit = false;
        m.bus.a_ecrit = false;
        m.bus.plancher_pile = 0;
        let debut = std::time::Instant::now();
        println!("\n  ---- {:#010x} ----", tete);
        'capture: while debut.elapsed().as_secs_f64() < 2.0 && vus < 8 {
            for _ in 0..20_000 {
                let avant = m.cpu.regs.pc;
                if !matches!(m.step(), StepResult::Ok(_)) {
                    break 'capture;
                }
                let apres = m.cpu.regs.pc;
                if apres != *tete || apres >= avant || avant - apres > PORTEE {
                    continue;
                }
                let r = &m.cpu.regs;
                let mut cur = [0u32; 17];
                cur[..13].copy_from_slice(&r.r);
                cur[13] = r.get_sp();
                cur[14] = r.lr;
                cur[15] = r.xpsr;
                cur[16] = r.itstate as u32;
                let ecrit = m.bus.a_ecrit;
                m.bus.a_ecrit = false;
                // Same rule as the core: stores below the head's stack pointer
                // do not count. Without that, the PUSH of the function called
                // on every iteration would mark everything as storing, and the
                // reading would say nothing.
                m.bus.plancher_pile = cur[13];
                if let Some(p) = precedent {
                    let mut diff = Vec::new();
                    for i in 0..17 {
                        if p[i] != cur[i] {
                            let nom = match i {
                                13 => "sp".to_string(),
                                14 => "lr".to_string(),
                                15 => "xpsr".to_string(),
                                16 => "it".to_string(),
                                n => format!("r{}", n),
                            };
                            diff.push(format!(
                                "{}={:#x} ({:+})",
                                nom,
                                cur[i],
                                cur[i].wrapping_sub(p[i]) as i32
                            ));
                        }
                    }
                    println!(
                        "    tour {} : {} ; {}",
                        vus,
                        if diff.is_empty() { "identique".to_string() } else { diff.join(", ") },
                        if precedent_ecrit { "avec rangement" } else { "sans rangement" }
                    );
                } else {
                    println!(
                        "    tour 0 : r0={:#x} r1={:#x} r2={:#x} r3={:#x} r4={:#x} r5={:#x} sp={:#x} lr={:#x}",
                        cur[0], cur[1], cur[2], cur[3], cur[4], cur[5], cur[13], cur[14]
                    );
                }
                precedent = Some(cur);
                precedent_ecrit = ecrit;
                vus += 1;
                if vus >= 8 {
                    break 'capture;
                }
            }
        }
        if vus == 0 {
            println!("    (pas repassee par la en deux secondes)");
        }
    }
}
