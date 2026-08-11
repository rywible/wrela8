P7 renderer actors boot across four cores as ordinary actors. The coordinator
rejects out-of-range and non-finite (infinity and NaN) frame input before
returning a deterministic debug frame for valid input, and a retry observes
the same success without reading kinetic state.
