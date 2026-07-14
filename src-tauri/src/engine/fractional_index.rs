pub fn generate_key_between(left: Option<&str>, right: Option<&str>) -> String {
    let left_str = left.unwrap_or("");
    let right_str = right.unwrap_or("");

    if left_str.is_empty() && right_str.is_empty() {
        return "m".to_string();
    }

    // Convert strings to vectors of values in 0..26 corresponding to 'a'..='z'
    let to_vals = |s: &str| -> Vec<i32> {
        s.chars()
            .map(|c| (c as i32) - ('a' as i32))
            .filter(|&v| (0..26).contains(&v))
            .collect()
    };

    let left_vals = to_vals(left_str);
    let right_vals = to_vals(right_str);

    let mut result_vals = Vec::new();
    let mut i = 0;
    let mut deviated_from_right = false;

    loop {
        let l_val = if i < left_vals.len() { left_vals[i] } else { 0 };
        let r_val = if deviated_from_right {
            26
        } else if i < right_vals.len() {
            right_vals[i]
        } else {
            26
        };

        if r_val > l_val + 1 {
            let mid = l_val + (r_val - l_val) / 2;
            result_vals.push(mid);
            break;
        } else {
            result_vals.push(l_val);
            if i < right_vals.len() && l_val < right_vals[i] {
                deviated_from_right = true;
            }
            i += 1;
        }
    }

    let mut result_str: String = result_vals.iter()
        .map(|&v| ((v + ('a' as i32)) as u8) as char)
        .collect();

    // Ensure we don't have trailing 'a's to avoid key bloat
    while result_str.ends_with('a') && result_str.len() > 1 {
        result_str.pop();
    }

    // Final safety checks:
    if let Some(l) = left {
        if result_str.as_str() <= l {
            result_str = format!("{}m", l);
        }
    }
    if let Some(r) = right {
        if result_str.as_str() >= r {
            if let Some(l) = left {
                result_str = format!("{}m", l);
            } else {
                result_str = format!("{}a", result_str);
            }
        }
    }

    result_str
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fractional_indexing() {
        assert_eq!(generate_key_between(None, None), "m");
        assert!(generate_key_between(Some("m"), None).as_str() > "m");
        assert!(generate_key_between(None, Some("m")).as_str() < "m");
        
        let mid1 = generate_key_between(Some("c"), Some("d"));
        assert!(mid1.as_str() > "c" && mid1.as_str() < "d");

        let mid2 = generate_key_between(Some("c"), Some("cx"));
        assert!(mid2.as_str() > "c" && mid2.as_str() < "cx");

        let mid3 = generate_key_between(Some("a"), Some("b"));
        assert!(mid3.as_str() > "a" && mid3.as_str() < "b");
    }
}
