//! Golden: granary_core::company equals backend/app/core/utils.py get_company_info
//! (compat-tests/golden/gen_company.py).

use granary_core::company::company_info_from_rows;
use serde_json::Value;

#[test]
fn company_info_matches_python() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../compat-tests/golden/company/cases.json"
    );
    let cases: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(cases.len(), 15);
    for c in &cases {
        let rows: Vec<(String, Option<String>)> = c["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r[0].as_str().unwrap().to_string(),
                    r[1].as_str().map(|s| s.to_string()),
                )
            })
            .collect();
        let got: Vec<(String, Value)> =
            company_info_from_rows(&rows).to_map().into_iter().collect();
        let want: Vec<(String, Value)> = c["result"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p[0].as_str().unwrap().to_string(), p[1].clone()))
            .collect();
        assert_eq!(got, want, "rows {:?}", c["rows"]);
    }
}
