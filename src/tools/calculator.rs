use super::{Tool, ToolContext, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

pub struct CalculatorTool;

#[async_trait]
impl Tool for CalculatorTool {
    fn name(&self) -> &str {
        "calculator"
    }

    fn description(&self) -> &str {
        "Perform mathematical calculations accurately. Supports arithmetic (add, subtract, multiply, divide, mod, pow, sqrt), logarithms (log, log_base, ln, exp), rounding (abs, floor, ceil, round), and statistics (average, median, variance, stdev_population, stdev_sample, min, max, count, percentile)."
    }

    fn parameters_json(&self) -> String {
        r#"{
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["add", "subtract", "multiply", "divide", "mod", "pow", "sqrt", "log", "log_base", "ln", "exp", "average", "median", "variance", "stdev_population", "stdev_sample", "min", "max", "count", "percentile", "abs", "floor", "ceil", "round"],
                    "description": "Calculation operation to perform"
                },
                "values": {
                    "type": "array",
                    "items": { "type": "number" },
                    "description": "Numeric values for the calculation. For ordered operations use left-to-right input order; log_base expects [value, base]."
                },
                "percentile_rank": {
                    "type": "integer",
                    "description": "Percentile rank 0-100, required for percentile operation"
                }
            },
            "required": ["operation", "values"]
        }"#.to_string()
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> Result<ToolResult> {
        let operation = match args.get("operation").and_then(|v| v.as_str()) {
            Some(op) => op,
            None => return Ok(ToolResult::fail("Missing required parameter: operation")),
        };

        let values = match args.get("values").and_then(|v| v.as_array()) {
            Some(v) => v,
            None => return Ok(ToolResult::fail("Missing required parameter: values")),
        };

        if values.is_empty() {
            return Ok(ToolResult::fail("values array must not be empty"));
        }

        let mut nums = Vec::with_capacity(values.len());
        for v in values {
            if let Some(n) = v.as_f64() {
                nums.push(n);
            } else {
                return Ok(ToolResult::fail("All values must be numbers"));
            }
        }

        match operation {
            "add" => {
                let sum: f64 = nums.iter().sum();
                Ok(ToolResult::ok(format_float(sum)))
            }
            "subtract" => {
                if nums.len() != 2 {
                    return Ok(ToolResult::fail("subtract requires exactly two values"));
                }
                Ok(ToolResult::ok(format_float(nums[0] - nums[1])))
            }
            "multiply" => {
                let product: f64 = nums.iter().product();
                Ok(ToolResult::ok(format_float(product)))
            }
            "divide" => {
                if nums.len() != 2 {
                    return Ok(ToolResult::fail("divide requires exactly two values"));
                }
                if nums[1] == 0.0 {
                    return Ok(ToolResult::fail("Division by zero"));
                }
                Ok(ToolResult::ok(format_float(nums[0] / nums[1])))
            }
            "mod" => {
                if nums.len() != 2 {
                    return Ok(ToolResult::fail("mod requires exactly two values"));
                }
                if nums[1] == 0.0 {
                    return Ok(ToolResult::fail("Modulo by zero"));
                }
                Ok(ToolResult::ok(format_float(nums[0] % nums[1])))
            }
            "pow" => {
                if nums.len() != 2 {
                    return Ok(ToolResult::fail("pow requires exactly two values"));
                }
                Ok(ToolResult::ok(format_float(nums[0].powf(nums[1]))))
            }
            "sqrt" => {
                if nums.len() != 1 {
                    return Ok(ToolResult::fail("sqrt requires exactly one value"));
                }
                if nums[0] < 0.0 {
                    return Ok(ToolResult::fail("sqrt of negative number"));
                }
                Ok(ToolResult::ok(format_float(nums[0].sqrt())))
            }
            "log" => {
                if nums.len() != 1 {
                    return Ok(ToolResult::fail("log requires exactly one value"));
                }
                if nums[0] <= 0.0 {
                    return Ok(ToolResult::fail("log of non-positive number"));
                }
                Ok(ToolResult::ok(format_float(nums[0].log10())))
            }
            "log_base" => {
                if nums.len() != 2 {
                    return Ok(ToolResult::fail(
                        "log_base requires exactly two values [value, base]",
                    ));
                }
                if nums[0] <= 0.0 || nums[1] <= 0.0 || nums[1] == 1.0 {
                    return Ok(ToolResult::fail("Invalid log base or value"));
                }
                Ok(ToolResult::ok(format_float(nums[0].log(nums[1]))))
            }
            "ln" => {
                if nums.len() != 1 {
                    return Ok(ToolResult::fail("ln requires exactly one value"));
                }
                if nums[0] <= 0.0 {
                    return Ok(ToolResult::fail("ln of non-positive number"));
                }
                Ok(ToolResult::ok(format_float(nums[0].ln())))
            }
            "exp" => {
                if nums.len() != 1 {
                    return Ok(ToolResult::fail("exp requires exactly one value"));
                }
                Ok(ToolResult::ok(format_float(nums[0].exp())))
            }
            "average" => {
                let sum: f64 = nums.iter().sum();
                Ok(ToolResult::ok(format_float(sum / nums.len() as f64)))
            }
            "median" => {
                nums.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let mid = nums.len() / 2;
                let median = if nums.len() % 2 == 0 {
                    (nums[mid - 1] + nums[mid]) / 2.0
                } else {
                    nums[mid]
                };
                Ok(ToolResult::ok(format_float(median)))
            }
            "min" => {
                let min = nums.iter().fold(f64::INFINITY, |a, &b| a.min(b));
                Ok(ToolResult::ok(format_float(min)))
            }
            "max" => {
                let max = nums.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));
                Ok(ToolResult::ok(format_float(max)))
            }
            "count" => Ok(ToolResult::ok(nums.len().to_string())),
            "abs" => {
                if nums.len() != 1 {
                    return Ok(ToolResult::fail("abs requires exactly one value"));
                }
                Ok(ToolResult::ok(format_float(nums[0].abs())))
            }
            "floor" => {
                if nums.len() != 1 {
                    return Ok(ToolResult::fail("floor requires exactly one value"));
                }
                Ok(ToolResult::ok(format_float(nums[0].floor())))
            }
            "ceil" => {
                if nums.len() != 1 {
                    return Ok(ToolResult::fail("ceil requires exactly one value"));
                }
                Ok(ToolResult::ok(format_float(nums[0].ceil())))
            }
            "round" => {
                if nums.len() != 1 {
                    return Ok(ToolResult::fail("round requires exactly one value"));
                }
                Ok(ToolResult::ok(format_float(nums[0].round())))
            }
            "variance" => {
                if nums.is_empty() {
                    return Ok(ToolResult::fail("Empty values"));
                }
                let mean = nums.iter().sum::<f64>() / nums.len() as f64;
                let var = nums.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / nums.len() as f64;
                Ok(ToolResult::ok(format_float(var)))
            }
            "stdev_population" => {
                if nums.is_empty() {
                    return Ok(ToolResult::fail("Empty values"));
                }
                let mean = nums.iter().sum::<f64>() / nums.len() as f64;
                let var = nums.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / nums.len() as f64;
                Ok(ToolResult::ok(format_float(var.sqrt())))
            }
            "stdev_sample" => {
                if nums.len() < 2 {
                    return Ok(ToolResult::fail("stdev_sample requires at least 2 values"));
                }
                let mean = nums.iter().sum::<f64>() / nums.len() as f64;
                let var =
                    nums.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / (nums.len() - 1) as f64;
                Ok(ToolResult::ok(format_float(var.sqrt())))
            }
            "percentile" => {
                let rank = match args.get("percentile_rank").and_then(|v| v.as_f64()) {
                    Some(r) if r >= 0.0 && r <= 100.0 => r,
                    _ => {
                        return Ok(ToolResult::fail(
                            "percentile_rank between 0-100 is required",
                        ));
                    }
                };
                nums.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let idx = (rank / 100.0) * (nums.len() - 1) as f64;
                let lower = idx.floor() as usize;
                let upper = (lower + 1).min(nums.len() - 1);
                let frac = idx - lower as f64;
                let val = nums[lower] + frac * (nums[upper] - nums[lower]);
                Ok(ToolResult::ok(format_float(val)))
            }
            _ => Ok(ToolResult::fail(format!(
                "Unknown operation: {}",
                operation
            ))),
        }
    }
}

fn format_float(val: f64) -> String {
    if val.is_nan() {
        return "NaN".to_string();
    }
    if val.is_infinite() {
        return if val.is_sign_positive() {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
    }

    let s = format!("{:.6}", val);
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}
