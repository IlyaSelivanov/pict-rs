use crate::combination::rng_srand;
use crate::exclusion::Exclusion;
use crate::model::{choose_and_link, Model};
use crate::parameter::Parameter;
use crate::task::{generate_recursive, Task};
use crate::types::{GenerationMode, GenerationType};
use crate::combination::Combination;

// ─────────────────────────────────────────────────────────────────────────────
// Helper: collect all pairwise combos covered by a result set
// ─────────────────────────────────────────────────────────────────────────────

/// Count how many unique (param_i, val_i, param_j, val_j) pairs appear in `rows`.
fn covered_pairs(rows: &[Vec<usize>]) -> std::collections::HashSet<(usize, usize, usize, usize)> {
    let mut pairs = std::collections::HashSet::new();
    if rows.is_empty() {
        return pairs;
    }
    let ncols = rows[0].len();
    for row in rows {
        for i in 0..ncols {
            for j in (i + 1)..ncols {
                pairs.insert((i, row[i], j, row[j]));
            }
        }
    }
    pairs
}

/// All possible pairwise combos for value counts `vc` (one entry per param).
fn all_pairs(vc: &[usize]) -> std::collections::HashSet<(usize, usize, usize, usize)> {
    let mut pairs = std::collections::HashSet::new();
    let n = vc.len();
    for i in 0..n {
        for j in (i + 1)..n {
            for vi in 0..vc[i] {
                for vj in 0..vc[j] {
                    pairs.insert((i, vi, j, vj));
                }
            }
        }
    }
    pairs
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_pairwise_3x3x3() {
    // 3 parameters, 3 values each — minimal pairwise
    let mut task = Task::new();
    let mut model = Model::new(String::new(), GenerationType::MixedOrder, 2, 0);

    for _ in 0..3 {
        let n = model.params.len();
        model.add_parameter(Parameter::new(2, (n + 1) as i32, 3, String::new(), false));
    }

    task.set_root_model(Box::new(model));
    task.generate().expect("generation failed");

    let results = task.results().unwrap();
    assert!(!results.is_empty(), "should produce at least one row");

    // Verify pairwise coverage
    let vc = vec![3usize, 3, 3];
    let expected = all_pairs(&vc);
    let actual = covered_pairs(results);
    for pair in &expected {
        assert!(
            actual.contains(pair),
            "pair {:?} not covered",
            pair
        );
    }
}

#[test]
fn test_pairwise_2x3x4() {
    let mut task = Task::new();
    let mut model = Model::new(String::new(), GenerationType::MixedOrder, 2, 0);

    let vc = [2, 3, 4];
    for &v in &vc {
        let n = model.params.len();
        model.add_parameter(Parameter::new(2, (n + 1) as i32, v, String::new(), false));
    }

    task.set_root_model(Box::new(model));
    task.generate().expect("generation failed");

    let results = task.results().unwrap();
    let expected = all_pairs(&vc.iter().map(|&x| x as usize).collect::<Vec<_>>());
    let actual = covered_pairs(results);
    for pair in &expected {
        assert!(actual.contains(pair), "pair {:?} not covered", pair);
    }
}

#[test]
fn test_pairwise_with_exclusion() {
    // 2 params, 3 values each; exclude (0, 0) ∧ (1, 0) — first values of both params
    let mut task = Task::new();
    let mut model = Model::new(String::new(), GenerationType::MixedOrder, 2, 0);

    model.add_parameter(Parameter::new(2, 1, 3, String::new(), false));
    model.add_parameter(Parameter::new(2, 2, 3, String::new(), false));

    let gid0 = model.params[0].global_id;
    let gid1 = model.params[1].global_id;

    // Exclusion: param0=0 AND param1=0
    let mut excl = Exclusion::new();
    excl.insert((gid0, 0));
    excl.insert((gid1, 0));
    task.add_exclusion(excl);

    task.set_root_model(Box::new(model));
    task.generate().expect("generation failed");

    let results = task.results().unwrap();
    // No row should have (0, 0)
    for row in results {
        assert!(
            !(row[0] == 0 && row[1] == 0),
            "excluded combination appeared in output"
        );
    }
}

#[test]
fn test_single_param_order1() {
    // 1 parameter, order=1: should enumerate all 4 values
    let mut task = Task::new();
    let mut model = Model::new(String::new(), GenerationType::MixedOrder, 1, 0);
    model.add_parameter(Parameter::new(1, 1, 4, String::new(), false));

    task.set_root_model(Box::new(model));
    task.generate().expect("generation failed");

    let results = task.results().unwrap();
    let seen: std::collections::HashSet<usize> = results.iter().map(|r| r[0]).collect();
    for v in 0..4 {
        assert!(seen.contains(&v), "value {} not in output", v);
    }
}

#[test]
fn test_single_param_pairwise_produces_nothing() {
    // 1 parameter with pairwise order=2 can't form any pairs — no output expected
    let mut task = Task::new();
    let mut model = Model::new(String::new(), GenerationType::MixedOrder, 2, 0);
    model.add_parameter(Parameter::new(2, 1, 4, String::new(), false));

    task.set_root_model(Box::new(model));
    task.generate().expect("generation failed");
    // Should succeed without crashing; result set may be empty
    let _ = task.results();
}

#[test]
fn test_two_params_two_values_full_coverage() {
    // 2 params × 2 values = 4 combinations; pairwise needs all 4
    let mut task = Task::new();
    let mut model = Model::new(String::new(), GenerationType::MixedOrder, 2, 0);

    model.add_parameter(Parameter::new(2, 1, 2, String::new(), false));
    model.add_parameter(Parameter::new(2, 2, 2, String::new(), false));

    task.set_root_model(Box::new(model));
    task.generate().expect("generation failed");

    let results = task.results().unwrap();
    assert!(!results.is_empty());

    let all = all_pairs(&[2, 2]);
    let got = covered_pairs(results);
    for p in &all {
        assert!(got.contains(p), "pair {:?} missing", p);
    }
}

#[test]
fn test_result_row_count_reasonable() {
    // 5 params × 5 values each: pairwise should be far fewer than 5^5=3125
    let mut task = Task::new();
    let mut model = Model::new(String::new(), GenerationType::MixedOrder, 2, 0);
    for _ in 0..5 {
        let n = model.params.len();
        model.add_parameter(Parameter::new(2, (n + 1) as i32, 5, String::new(), false));
    }
    task.set_root_model(Box::new(model));
    task.generate().expect("generation failed");

    let results = task.results().unwrap();
    // Optimal pairwise for 5×5^4 is around 25 rows; we allow generous upper bound
    assert!(results.len() <= 200, "too many rows: {}", results.len());
    assert!(!results.is_empty());

    let vc = vec![5usize; 5];
    let expected = all_pairs(&vc);
    let actual = covered_pairs(results);
    for p in &expected {
        assert!(actual.contains(p), "pair {:?} not covered", p);
    }
}
