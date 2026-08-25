# Oscillator

`dsp::nco` generates the complex exponential the mixer multiplies
against. Three things about it shape everything downstream.

## The phase accumulator is never reset

`master` is absolute elapsed phase, not phase since the last trigger.
That is the whole basis of the chain being *phase sensitive*: two
acquisitions taken minutes apart are comparable because they share one
phase reference. Resetting the accumulator at each acquisition would
make every measurement self-referential and mutually useless.

## Sine and cosine come from a table plus interpolation

A 48-bit phase accumulator, truncated to 22 bits of table index, with
linear interpolation over the low bits. The truncation is deliberate
and its cost is quantified: phase truncation is a *deterministic* error
that appears as spurs at predictable frequencies, which is far easier
to live with than the noise floor a dithered accumulator would give.

## It marks its own samples

When the frequency or phase word changes, the NCO tags the first output
sample affected. Downstream widgets carry that mark rather than
inferring timing — see `dsp::sync` and the marker discussion in
[The Down-Converter](ddc.md).

The latency from a control write to the marked sample is *published*
(`latency::FREQUENCY_CONTROL` and friends) rather than left for the
integrator to measure, so a controller can time a change to land on a
chosen sample.

```admonish note title="Full detail is in the rustdoc"
Each widget's own documentation carries its schematic symbol, internal
block diagram, a runnable example and a committed waveform trace. This
chapter is the map, not the territory — see `rhdl_fpga::dsp::nco`.
```
