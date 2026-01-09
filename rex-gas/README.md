# Rex Gas (`rex-gas`)

This crate provides a small “gas” metering abstraction used to bound work in the parser, type
inference, and evaluator.

## API

- `GasMeter`: tracks remaining budget and charges for operations
- `GasCosts`: a struct of per-operation costs (with sensible defaults)
- `OutOfGas`: error returned when the budget is exhausted

The intent is to make it practical to run untrusted or adversarial inputs without relying on wall
clock timeouts alone.

