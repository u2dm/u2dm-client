const FIRST: u8 = b'!';
const LAST: u8 = b'~';
const BASE: u32 = (LAST - FIRST + 1) as u32;
const MAX_ORDER_LEN: usize = 50;
const MAX_SEED_WIDTH: usize = 8;

fn to_digit(byte: u8) -> Option<u32> {
    (FIRST..=LAST)
        .contains(&byte)
        .then(|| u32::from(byte - FIRST))
}

fn from_digit(digit: u32) -> u8 {
    FIRST.saturating_add(u8::try_from(digit % BASE).unwrap_or(0))
}

fn digits(order: &str) -> Vec<u32> {
    order.bytes().filter_map(to_digit).collect()
}

fn to_order(digits: &[u32]) -> String {
    digits.iter().map(|&d| char::from(from_digit(d))).collect()
}

fn midpoint(a: &str, b: &str) -> String {
    let low = digits(a);
    let high = digits(b);
    let len = low.len().max(high.len());
    let digit_at = |src: &[u32], i: usize| src.get(i).copied().unwrap_or(0);

    let mut sum = vec![0u32; len + 1];
    let mut carry = 0u32;
    for i in (0..len).rev() {
        let column = digit_at(&low, i) + digit_at(&high, i) + carry;
        if let Some(slot) = sum.get_mut(i + 1) {
            *slot = column % BASE;
        }
        carry = column / BASE;
    }
    if let Some(slot) = sum.get_mut(0) {
        *slot = carry;
    }

    let mut halved = vec![0u32; len + 1];
    let mut remainder = 0u32;
    for i in 0..=len {
        let column = remainder * BASE + digit_at(&sum, i);
        if let Some(slot) = halved.get_mut(i) {
            *slot = column / 2;
        }
        remainder = column % 2;
    }

    let mut result: Vec<u32> = halved.into_iter().skip(1).collect();
    if remainder == 1 {
        result.push(BASE / 2);
    }
    to_order(&result)
}

fn smallest_order() -> String {
    to_order(&[0])
}

fn order_above(a: &str) -> String {
    to_order(&vec![BASE - 1; a.len() + 1])
}

pub fn between(a: Option<&str>, b: Option<&str>) -> Option<String> {
    let candidate = match (a, b) {
        (Some(a), Some(b)) => midpoint(a, b),
        (None, Some(b)) => midpoint(&smallest_order(), b),
        (Some(a), None) => midpoint(a, &order_above(a)),
        (None, None) => to_order(&[BASE / 2]),
    };

    let above_low = a.is_none_or(|a| a < candidate.as_str());
    let below_high = b.is_none_or(|b| candidate.as_str() < b);
    (above_low && below_high && !candidate.is_empty() && candidate.len() <= MAX_ORDER_LEN)
        .then_some(candidate)
}

fn encode_fixed_width(mut value: u128, width: usize) -> String {
    let base = u128::from(BASE);
    let mut digits = vec![0u32; width];
    for slot in digits.iter_mut().rev() {
        *slot = u32::try_from(value % base).unwrap_or(0);
        value /= base;
    }
    to_order(&digits)
}

pub fn even_orders(n: usize) -> Vec<String> {
    if n == 0 {
        return Vec::new();
    }

    let needed = u128::try_from(n).unwrap_or(u128::MAX).saturating_add(2);
    let mut width = 1usize;
    let mut capacity = u128::from(BASE);
    while capacity < needed && width < MAX_SEED_WIDTH {
        width += 1;
        capacity = capacity.saturating_mul(u128::from(BASE));
    }

    let slots = u128::try_from(n).unwrap_or(u128::MAX).saturating_add(1);
    let step = (capacity / slots).max(1);
    (0..n)
        .map(|i| {
            let rank = u128::try_from(i).unwrap_or(0).saturating_add(1);
            encode_fixed_width(rank.saturating_mul(step), width)
        })
        .collect()
}
