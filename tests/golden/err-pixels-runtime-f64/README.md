# Runtime f64 renderer predicates

The typed program is valid, but P2 accepts no runtime `f64` arithmetic or
comparison. Both image sealing and symbolic graph compilation must fail with
P003 before a partial graph is emitted.
