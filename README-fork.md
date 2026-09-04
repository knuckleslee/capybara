# What this fork changes

A fork of [infinition/capybara](https://github.com/infinition/capybara), a
bare-metal emulator for the Sonix SNC73410 running Tamagotchi Paradise firmware.
Everything upstream does, this does; the changes below are additions and fixes on
top of 0.2.2.

## Speed and smoothness

On a water-edition image the console ran at 0.69× real time and the interface at
about twenty frames per second. It now holds real time with core time to spare,
and the interface follows the display's refresh rate.

| | upstream 0.2.2 | this fork |
|---|---|---|
| console speed | 0.69× | 1.00× |
| interface | ~22 fps | display refresh rate |
| interpreter throughput | 66.1 M cycles/s | unchanged |

The interpreter was not made faster. Two things changed.

**The firmware's waiting is no longer interpreted.** Two thirds of console time
goes into a four-instruction loop waiting for the display's tearing-effect
signal. The core now recognises that kind of loop and advances the clock instead
of unrolling it. Peripherals keep being serviced exactly as before; only the
firmware's observation of them is delayed, by at most 85 microseconds.

**Emulation no longer shares a thread with drawing.** A frame used to cost the
emulation slice plus the drawing, however fast the core ran. A worker thread now
owns the machine and the interface reads a mirror. Periodic disk writes moved off
the emulation loop too, for the same reason.

Both are reversible at runtime without recompiling:

```
CAPYBARA_SANS_REPOS=1     restores the original core
CAPYBARA_UN_SEUL_FIL=1    restores the single-threaded loop
```

## Input fixes

Three defects present in 0.2.2:

- Hovering the mouse over the window discarded all keyboard input, silently.
- The wheel's encoder queue drained slower than keyboard repeat filled it, so
  input piled up and was eventually thrown away in one go.
- `tourner_molette` ignored the magnitude of a turn.

The wheel also no longer borrows the system's keyboard repeat, whose delay is
long and whose rate differs from machine to machine. It follows a curve that
accelerates, which is what makes a dial feel like a dial.

## Three new settings

In the right-click menu, all off by default.

**Keep the console awake.** The firmware sleeps after a few idle minutes and the
screen goes dark. This clears its idle counter so it never decides to sleep —
nothing is forced, no scene is short-circuited, and there is no visible wake.

The counter's address depends on the firmware edition. The default suits the
water edition; for another, `inactivite_probe` finds it and the address is given
without recompiling:

```
CAPYBARA_COMPTEUR_INACTIVITE=0x18001bfe:2
```

**Pause the console when closed.** Saves carry a timestamp and the gap is added
to the seconds counter on reopening, so a Tamagotchi left alone ages, as on the
real device. This makes the counter resume where it stopped.

**Lock on a recovery point.** A padlock per row in the recovery-point list. A
locked point survives pruning and the twelve-hour maximum age until deleted by
hand, and does not count towards interval spacing — otherwise locking one would
delete its neighbour.

## Three new probes

In the style of the fifty-four already present:

| probe | what it does |
|---|---|
| `repos_probe` | throughput with and without the fast-forward, and the share of time skipped |
| `boucle_probe` | the hottest short loops, disassembled, with what changes between iterations |
| `inactivite_probe` | finds the idle counter by its signature: rises while idle, falls on a press |

`inactivite_probe` is the tool for porting the no-sleep setting to another
firmware edition.

## Notes

`Cargo.toml` is untouched — no new dependencies. Comments on the changed code are
in English; upstream's are in French.

The reasoning behind each change sits in the comments next to it, including the
uncertainties. The main one: the idle-counter address `0x18001BFE` comes from a
measured signature and from five idle minutes without sleeping, not from
disassembly showing the firmware reading it.

## Licence

GPL-3.0-only, as upstream. Original work by
[infinition](https://github.com/infinition/capybara).
