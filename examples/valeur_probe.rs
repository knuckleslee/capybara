//! Watches one word of memory and records every change on the way to the
//! shutdown.
//!
//! Usage: cargo run --release --example valeur_probe --
//!            <dump.bin> <hex key> [state.tamastate] [place]
//!
//! The full trace showed the decision: at `0x10030C94` the firmware compares
//! the first word of the structure at `[0x18014038]` against one thousand, and
//! winds down when they are equal. Everything found before that — four gates
//! reading single bits — turned out to be routing that happens afterwards.
//!
//! But the word already held one thousand throughout the sixty thousand
//! instructions recorded, so the trace could not say who put it there. This
//! probe watches that word for the whole run instead of a window at the end,
//! and records every change with the instruction that made it and the call it
//! was made from.
//!
//! The place defaults to `*0x18014038+0`, which is the word the comparison
//! reads; give another to follow a different lead. Watching costs one read per
//! instruction, so this runs slower than the console does, and a minute of
//! console time takes several.
//!
//! Every change is reported with the registers as they stood, because the
//! instruction alone is rarely enough: the word that decides the shutdown is
//! cleared by a bulk store in PRAM, and only its base and count say whether the
//! structure was meant to be cleared or was merely in the way.
//!
//! Checking after every instruction is what makes the instruction knowable, and
//! it is slow. So the run is done twice: once a frame at a time, which goes at
//! something near the console's own speed and finds when each change happens,
//! then again instruction by instruction but only over the stretch that
//! matters, entering the slow mode a few million cycles before the first change
//! and leaving the rest at speed.
//!
//! The cost is therefore set by the distance between changes, not by how long
//! the console has been running, and a save left to idle for a quarter of an
//! hour is examined in about as long as it takes to reach that point once.
//!
//! `CAPYBARA_RAPIDE=1` skips the second pass, for when the sequence of changes
//! is all that is wanted.

use capybara::emulator::etat::Instantane;
use capybara::emulator::{Disassembler, Machine, StepResult};

/// Entry of the routine that winds the console down.
const ARRET: u32 = 0x1002_4018;

/// The registers before an instruction, and after it where they differ.
fn etat(m: &Machine, avant: &[u32; 16]) -> String {
    let mut sortie = String::new();
    for i in 0..15 {
        let apres = match i {
            13 => m.cpu.regs.get_sp(),
            14 => m.cpu.regs.lr,
            n => m.cpu.regs.get_reg(n as u8),
        };
        let nom = match i {
            13 => "sp".to_string(),
            14 => "lr".to_string(),
            n => format!("r{n}"),
        };
        if apres == avant[i] {
            sortie.push_str(&format!("{nom}={:#x} ", avant[i]));
        } else {
            sortie.push_str(&format!("{nom}={:#x}->{:#x} ", avant[i], apres));
        }
    }
    sortie
}

/// Loads the image, and a saved state when one is given.
fn charger(
    path: &str,
    key: u32,
    etat_path: &Option<String>,
    _p: u32,
    _d: u32,
    _f: Option<u32>,
) -> Machine {
    let mut m = Machine::new();
    m.device_key = Some(key);
    m.load_firmware_file(path).unwrap();
    if let Some(p) = etat_path.as_deref() {
        if !p.is_empty() && p != "-" {
            let etat = Instantane::lire(std::path::Path::new(p)).expect("reading the state");
            m.restaurer(&etat);
        }
    }
    m.bus.mmio_trace.enabled = false;
    m.is_running = true;
    m
}

/// Return addresses still on the stack, innermost first.
///
/// The instruction that writes a value is often a shared helper, and its link
/// register only names the helper's own caller. The chain that matters is
/// further out, and it is still lying on the stack: any word there that looks
/// like a code address with the Thumb bit set is a return address left by a
/// call that has not come back yet.
fn retours(m: &mut Machine) -> Vec<u32> {
    let sp = m.cpu.regs.get_sp();
    let mut v = Vec::new();
    for k in 0..96u32 {
        let adresse = sp.wrapping_add(k * 4);
        let mot = m.bus.read_u32(adresse, &mut m.periph, &m.cpu.nvic);
        let cible = mot & !1;
        if (mot & 1) != 0 && (cible <= 0x0000_FFFF || (0x1000_0000..=0x100F_FFFF).contains(&cible)) {
            v.push(cible);
        }
    }
    v
}

/// The address the place names now, and what it holds.
fn lire_lieu(m: &Machine, pointeur: u32, decalage: u32, fixe: Option<u32>) -> Option<(u32, u32)> {
    let adresse = match fixe {
        Some(a) => a,
        None => {
            let base = m.lire_mot_sram(pointeur);
            if !(0x1800_0000..0x1802_0000).contains(&base) {
                return None;
            }
            base.wrapping_add(decalage)
        }
    };
    Some((adresse, m.lire_mot_sram(adresse)))
}

fn main() {
    let mut a = std::env::args().skip(1);
    let path = a.next().expect("dump.bin");
    let key = u32::from_str_radix(a.next().expect("hex key").trim_start_matches("0x"), 16).unwrap();
    let etat_path = a.next();
    let lieu = a.next().unwrap_or_else(|| "*0x18014038+0".to_string());

    let hexa = |v: &str| u32::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok();
    let (pointeur, decalage, fixe) = if let Some(reste) = lieu.trim().strip_prefix('*') {
        let (p, d) = reste.split_once('+').unwrap_or((reste, "0"));
        (hexa(p).expect("pointer"), d.trim().parse::<u32>().unwrap_or(0), None)
    } else {
        (0, 0, Some(hexa(&lieu).expect("address")))
    };

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

    println!("  watching {lieu} until the core reaches {ARRET:#010x}\n");
    println!(
        "  {:<14} {:<12} {:<12} {:<12} {}",
        "cycle", "at", "address", "was", "became"
    );

    let lire = |m: &Machine| lire_lieu(m, pointeur, decalage, fixe);

    let rapide = std::env::var("CAPYBARA_RAPIDE").is_ok_and(|v| v.trim() != "0");
    // How far ahead of the first change to slow down. A few million cycles is a
    // few hundredths of a second of console time: ample for the instruction to
    // be caught, brief enough that the slow pass costs nothing.
    const MARGE: u64 = 4_000_000;
    let mut lent_a_partir_de = 0u64;
    if !rapide {
        println!("  first pass at speed, to find when it changes...");
        let mut essai = charger(&path, key, &etat_path, pointeur, decalage, fixe);
        let mut vu = lire_lieu(&essai, pointeur, decalage, fixe);
        let butoir = essai.cpu.cycles + 96_000_000u64 * 1200;
        let mut trouve = None;
        while essai.cpu.cycles < butoir {
            if essai.cpu.regs.pc == ARRET {
                break;
            }
            if !matches!(essai.run_frame(), StepResult::Ok(_)) {
                break;
            }
            let a = lire_lieu(&essai, pointeur, decalage, fixe);
            if a != vu {
                trouve = Some(essai.cpu.cycles);
                break;
            }
            vu = a;
        }
        match trouve {
            Some(c) => {
                lent_a_partir_de = c.saturating_sub(MARGE);
                println!(
                    "  it first changes at {c} cycles; slowing down from {lent_a_partir_de}\n"
                );
            }
            None => {
                println!("  it never changed in twenty minutes of console time.\n");
                return;
            }
        }
    } else {
        println!("  one check per frame: the cycle is known to within a frame, the");
        println!("  instruction not at all. Registers are those at the end of the frame.\n");
    }
    println!("  starting at {} cycles", m.cpu.cycles);
    let mut precedent = lire(&m);
    let mut changements = 0;
    // Twenty minutes of console time: long enough for a saved game left to idle,
    // which is the case this was written for.
    let plafond = m.cpu.cycles + 96_000_000u64 * 1200;

    while m.cpu.cycles < plafond {
        if m.cpu.regs.pc == ARRET {
            println!("\n  reached the wind-down at {} cycles", m.cpu.cycles);
            break;
        }
        let pc = m.cpu.regs.pc;
        let lr = m.cpu.regs.lr;
        // Kept from before the step: a bulk store advances its own base as it
        // goes, so the values afterwards say where it finished, not where it
        // started, and it is the start that matters.
        let mut avant = [0u32; 16];
        for (i, v) in avant.iter_mut().enumerate().take(13) {
            *v = m.cpu.regs.get_reg(i as u8);
        }
        avant[13] = m.cpu.regs.get_sp();
        avant[14] = lr;
        avant[15] = pc;
        // At speed until the interesting stretch, then one instruction at a
        // time so that the instruction and its registers can be named.
        let avance = if rapide || m.cpu.cycles < lent_a_partir_de {
            matches!(m.run_frame(), StepResult::Ok(_))
        } else {
            matches!(m.step(), StepResult::Ok(_))
        };
        if !avance {
            println!("\n  the core stopped first");
            break;
        }
        let actuel = lire(&m);
        if actuel != precedent {
            match (precedent, actuel) {
                (Some((_, av)), Some((ad, ap))) if av != ap => {
                    println!(
                        "  {:<14} {:#010x}   {:#010x}   {:#010x}   {:#010x}   from {:#010x}",
                        m.cpu.cycles, pc, ad, av, ap, lr
                    );
                    println!("        registers: {}", etat(&m, &avant));
                    let r = retours(&mut m);
                    println!(
                        "        return addresses on the stack: {}",
                        r.iter().map(|a| format!("{a:#010x}")).collect::<Vec<_>>().join(" ")
                    );
                    changements += 1;
                }
                (None, Some((ad, ap))) => {
                    println!(
                        "  {:<14} {:#010x}   {:#010x}   {:>10}   {:#010x}   from {:#010x}",
                        m.cpu.cycles, pc, ad, "(none)", ap, lr
                    );
                    println!("        registers: {}", etat(&m, &avant));
                    changements += 1;
                }
                (Some((ad, av)), None) => {
                    println!(
                        "  {:<14} {:#010x}   {:#010x}   {:#010x}   {:>10}",
                        m.cpu.cycles, pc, ad, av, "(gone)"
                    );
                    changements += 1;
                }
                _ => {}
            }
            // The code around the change, read now. The execute-in-place
            // window is programmable, so a listing taken by another tool at
            // another moment shows different instructions; only a reading taken
            // here, with the machine as it stood, can be trusted.
            //
            //   CAPYBARA_DESASSEMBLER=0x10054c30-0x10054c70
            for plage in std::env::var("CAPYBARA_DESASSEMBLER")
                .unwrap_or_default()
                .split(',')
                .filter(|p| !p.trim().is_empty())
            {
                let h = |v: &str| u32::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok();
                let Some((a, b)) = plage.split_once('-') else {
                    continue;
                };
                let (Some(d0), Some(d1)) = (h(a), h(b)) else {
                    continue;
                };
                println!("\n        {d0:#010x} to {d1:#010x}, as the window stands now:\n");
                let mut adresse = d0 & !1;
                while adresse < d1 {
                    let w1 = m.bus.read_u16(adresse, &mut m.periph, &m.cpu.nvic);
                    let w2 = m.bus.read_u16(adresse.wrapping_add(2), &mut m.periph, &m.cpu.nvic);
                    let d = Disassembler::disassemble(adresse, &[w1, w2]);
                    println!(
                        "        {} {:#010x}   {:04X}{}   {:<10} {}",
                        if adresse == pc { "->" } else { "  " },
                        adresse,
                        w1,
                        if d.is_32bit { format!(" {w2:04X}") } else { "     ".to_string() },
                        d.mnemonic,
                        d.operands
                    );
                    adresse += if d.is_32bit { 4 } else { 2 };
                }
                println!();
            }
            precedent = actuel;
        }
    }

    println!("\n  {changements} changes in all.");
    if let Some((ad, v)) = lire(&m) {
        println!("  it holds {v:#x} at {ad:#010x} now.");
    }
    println!(
        "\n  The line whose \"became\" is 0x3e8 is the one that decided: the address\n  \
         under \"at\" is the instruction that wrote it, and \"from\" is where that\n  \
         function was called."
    );
}
