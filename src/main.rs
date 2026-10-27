const PRODUCT_NAME: &str = "Cognitive Popups";

fn main() {
    println!("{PRODUCT_NAME} — development scaffold (no desktop UI yet)");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_identity_is_stable() {
        assert_eq!(PRODUCT_NAME, "Cognitive Popups");
    }
}
