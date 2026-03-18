use std::fmt;

#[derive(Clone, Debug)]
pub struct GasCosts {
    pub parse_token: u64,
    pub parse_node: u64,
    pub infer_node: u64,
    pub unify_step: u64,
    pub eval_node: u64,
    pub eval_app_step: u64,
    pub eval_match_arm: u64,
    pub eval_record_update_field: u64,
    pub native_call_base: u64,
    pub native_call_per_arg: u64,
}

impl GasCosts {
    pub fn sensible_defaults() -> Self {
        Self {
            parse_token: 1,
            parse_node: 2,
            infer_node: 5,
            unify_step: 2,
            eval_node: 3,
            eval_app_step: 1,
            eval_match_arm: 1,
            eval_record_update_field: 1,
            native_call_base: 10,
            native_call_per_arg: 2,
        }
    }
}

impl Default for GasCosts {
    fn default() -> Self {
        Self::sensible_defaults()
    }
}

#[derive(Clone, Debug)]
pub struct GasMeter {
    remaining: Option<u64>,
    pub costs: GasCosts,
}

impl GasMeter {
    pub fn new(remaining: Option<u64>, costs: GasCosts) -> Self {
        Self { remaining, costs }
    }

    pub fn unlimited(costs: GasCosts) -> Self {
        Self {
            remaining: None,
            costs,
        }
    }

    pub fn remaining(&self) -> Option<u64> {
        self.remaining
    }

    pub fn charge(&mut self, amount: u64) -> Result<(), OutOfGas> {
        let Some(mut remaining) = self.remaining else {
            return Ok(());
        };
        if remaining < amount {
            return Err(OutOfGas {
                needed: amount,
                remaining,
            });
        }
        remaining -= amount;
        self.remaining = Some(remaining);
        Ok(())
    }
}

impl Default for GasMeter {
    fn default() -> Self {
        Self::unlimited(GasCosts::default())
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("out of gas (needed {needed}, remaining {remaining})")]
pub struct OutOfGas {
    pub needed: u64,
    pub remaining: u64,
}

impl fmt::Display for GasMeter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.remaining {
            None => write!(f, "GasMeter(unlimited)"),
            Some(r) => write!(f, "GasMeter(remaining={r})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gas_meter_default_is_unlimited() {
        let meter = GasMeter::default();
        assert_eq!(meter.remaining(), None);
        assert_eq!(
            meter.costs.parse_token,
            GasCosts::sensible_defaults().parse_token
        );
    }

    #[test]
    fn gas_meter_default_uses_default_costs() {
        let meter = GasMeter::default();
        let expected = GasMeter::default();
        assert_eq!(meter.remaining(), expected.remaining());
        assert_eq!(meter.costs.eval_node, expected.costs.eval_node);
    }
}
