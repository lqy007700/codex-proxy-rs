//! 按冻结的管理员 USD 单价计算智谱标准 API 费用，不推测上游人民币账单

use gateway_core::metering::{
    CalculatedCost, CalculatedCostAmounts, CalculatedCostBreakdown, CalculatedCostRates,
    ModelPriceOverride, Usage,
};

pub(crate) fn breakdown(
    usage: &Usage,
    pricing: Option<&ModelPriceOverride>,
) -> Option<CalculatedCostBreakdown> {
    if usage
        .cache_write_input_tokens
        .is_some_and(|tokens| tokens > 0)
        || usage.image_input_tokens.is_some_and(|tokens| tokens > 0)
        || usage.image_output_tokens.is_some_and(|tokens| tokens > 0)
    {
        return None;
    }
    let pricing = pricing?;
    let rates = pricing.bands.get("standard")?;
    let input = usage.input_tokens?;
    let output = usage.output_tokens?;
    let cache = usage.cached_input_tokens?;
    let input = input.checked_sub(cache)?;
    let input_ticks = u128::from(input).checked_mul(rates.input.ticks_per_token())?;
    let output_ticks = u128::from(output).checked_mul(rates.output.ticks_per_token())?;
    let cache_ticks = u128::from(cache).checked_mul(rates.cache_read.ticks_per_token())?;
    let standard = input_ticks
        .checked_add(output_ticks)?
        .checked_add(cache_ticks)?;
    let money = |ticks| {
        CalculatedCost::from_usd_ticks(ticks)
            .ok()
            .map(|cost| cost.total())
    };
    CalculatedCostBreakdown::new(
        CalculatedCostAmounts::new(
            money(input_ticks)?,
            money(output_ticks)?,
            money(cache_ticks)?,
            money(0)?,
            money(standard)?,
            money(standard)?,
        ),
        CalculatedCostRates::new(
            money(rates.input.ticks_per_token().checked_mul(1_000_000)?)?,
            money(rates.output.ticks_per_token().checked_mul(1_000_000)?)?,
            money(rates.cache_read.ticks_per_token().checked_mul(1_000_000)?)?,
            money(rates.cache_write.ticks_per_token().checked_mul(1_000_000)?)?,
        ),
        None,
        100,
    )
    .with_custom_multiplier(pricing.multiplier_bps)
}
