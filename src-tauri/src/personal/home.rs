// SPDX-License-Identifier: AGPL-3.0-only
//! Health and home: food and workouts, the health log, recipes and meal
//! plans, the car, plants and pets. Numbers the user typed in, summed and
//! compared; no medical advice is drawn from them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

const DAY: i64 = 86_400_000;

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct DayTotals {
    pub calories: f64,
    pub protein_g: f64,
    pub workout_min: f64,
}

/// Food and workout entries summed for one day; missing fields count as zero.
pub fn day_totals(entries: &[serde_json::Value]) -> DayTotals {
    let n = |v: &serde_json::Value, k: &str| v[k].as_f64().unwrap_or(0.0);
    entries.iter().fold(DayTotals::default(), |mut t, e| {
        t.calories += n(e, "calories");
        t.protein_g += n(e, "protein_g");
        t.workout_min += n(e, "minutes");
        t
    })
}

/// The last week's average against the week before, for one measure.
/// `None` until both weeks have readings.
pub fn weekly_change(readings: &[(i64, f64)], now_ms: i64) -> Option<f64> {
    let avg = |from: i64, to: i64| {
        let v: Vec<f64> = readings.iter().filter(|(t, _)| *t > from && *t <= to).map(|(_, x)| *x).collect();
        (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
    };
    Some(avg(now_ms - 7 * DAY, now_ms)? - avg(now_ms - 14 * DAY, now_ms - 7 * DAY)?)
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Ingredient {
    pub name: String,
    pub qty: f64,
    pub unit: String,
}

pub fn scale(ingredients: &[Ingredient], from_servings: f64, to_servings: f64) -> Vec<Ingredient> {
    let f = if from_servings > 0.0 { to_servings / from_servings } else { 1.0 };
    ingredients.iter().map(|i| Ingredient { qty: (i.qty * f * 100.0).round() / 100.0, ..i.clone() }).collect()
}

/// Same name and unit add up, compared without case, so a week of meals
/// gives one shopping list.
pub fn shopping_list(meals: &[Vec<Ingredient>]) -> Vec<Ingredient> {
    let mut m: BTreeMap<(String, String), f64> = BTreeMap::new();
    for i in meals.iter().flatten() {
        *m.entry((i.name.trim().to_lowercase(), i.unit.trim().to_lowercase())).or_default() += i.qty;
    }
    m.into_iter().map(|((name, unit), qty)| Ingredient { name, qty, unit }).collect()
}

/// Whichever comes first, distance or time.
pub fn service_due(last_km: f64, now_km: f64, every_km: f64, last_ms: i64, now_ms: i64, every_days: i64) -> bool {
    now_km - last_km >= every_km || now_ms - last_ms >= every_days * DAY
}

/// Plants and pets whose last care plus interval has passed.
/// Items are (name, last_ms, every_days).
pub fn due_care(items: &[(String, i64, i64)], now_ms: i64) -> Vec<String> {
    items.iter().filter(|(_, last, every)| now_ms - last >= every * DAY).map(|(n, _, _)| n.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn food_and_workouts_sum() {
        let t = day_totals(&[json!({"calories": 500, "protein_g": 30}), json!({"minutes": 45}), json!({"calories": 250.5})]);
        assert_eq!(t, DayTotals { calories: 750.5, protein_g: 30.0, workout_min: 45.0 });
    }

    #[test]
    fn health_trend_needs_two_weeks() {
        let now = 20 * DAY;
        let r = [(now - 10 * DAY, 80.0), (now - 8 * DAY, 82.0), (now - DAY, 79.0)];
        assert_eq!(weekly_change(&r, now), Some(-2.0));
        assert_eq!(weekly_change(&r[2..], now), None);
    }

    #[test]
    fn recipes_scale_and_merge() {
        let i = |n: &str, q: f64, u: &str| Ingredient { name: n.into(), qty: q, unit: u.into() };
        assert_eq!(scale(&[i("flour", 300.0, "g")], 4.0, 6.0)[0].qty, 450.0);
        let list = shopping_list(&[vec![i("Egg", 2.0, "")], vec![i("egg ", 3.0, ""), i("milk", 1.0, "l")]]);
        assert_eq!(list, vec![i("egg", 5.0, ""), i("milk", 1.0, "l")]);
    }

    #[test]
    fn car_and_care_due() {
        assert!(service_due(10_000.0, 20_000.0, 10_000.0, 0, 0, 365));
        assert!(service_due(0.0, 10.0, 10_000.0, 0, 365 * DAY, 365));
        assert!(!service_due(0.0, 10.0, 10_000.0, 0, DAY, 365));
        let items = [("Fern".to_string(), 0, 3), ("Cat flea dose".to_string(), 0, 30)];
        assert_eq!(due_care(&items, 3 * DAY), ["Fern"]);
    }
}
