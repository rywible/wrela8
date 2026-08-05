P5 renderer actors boot across four cores as ordinary actors. The coordinator
rejects out-of-range and non-finite (infinity and NaN) frame input before
returning the sealed `RendererUnavailable` contract for valid input, and a
retry observes the same result without presentation.
