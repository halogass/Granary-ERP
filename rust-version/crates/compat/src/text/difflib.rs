//! `difflib.SequenceMatcher(None, a, b)` over code points, line for line with CPython 3.12: b2j, the
//! autojunk "popular element" purge for len(b) >= 200, find_longest_match with its extension loops,
//! get_matching_blocks (queue order, sort, collapse) and ratio / quick_ratio / real_quick_ratio.

use std::collections::HashMap;

use rust_decimal::prelude::*;

/// A matching block: a[a..a+size] == b[b..b+size].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Match {
    pub a: usize,
    pub b: usize,
    pub size: usize,
}

pub struct SequenceMatcher {
    a: Vec<char>,
    b: Vec<char>,
    b2j: HashMap<char, Vec<usize>>,
}

impl SequenceMatcher {
    /// `SequenceMatcher(None, a, b)` (autojunk=True).
    pub fn new(a: &str, b: &str) -> Self {
        let a: Vec<char> = a.chars().collect();
        let b: Vec<char> = b.chars().collect();
        let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
        for (i, &elt) in b.iter().enumerate() {
            b2j.entry(elt).or_default().push(i);
        }
        let n = b.len();
        if n >= 200 {
            let ntest = n / 100 + 1;
            b2j.retain(|_, idxs| idxs.len() <= ntest);
        }
        SequenceMatcher { a, b, b2j }
    }

    pub fn find_longest_match(&self, alo: usize, ahi: usize, blo: usize, bhi: usize) -> Match {
        let (a, b) = (&self.a, &self.b);
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        #[allow(clippy::needless_range_loop)] // the indices are difflib's own (i, j)
        for i in alo..ahi {
            let mut newj2len: HashMap<usize, usize> = HashMap::new();
            if let Some(js) = self.b2j.get(&a[i]) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = (if j > 0 {
                        j2len.get(&(j - 1)).copied().unwrap_or(0)
                    } else {
                        0
                    }) + 1;
                    newj2len.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = newj2len;
        }
        // isjunk is None, so bjunk is empty: the "not isbjunk" loops extend over any equal element
        // (popular ones included) and the "isbjunk" loops never run.
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && a[besti + bestsize] == b[bestj + bestsize]
        {
            bestsize += 1;
        }
        Match {
            a: besti,
            b: bestj,
            size: bestsize,
        }
    }

    /// `get_matching_blocks()`, including the (len(a), len(b), 0) sentinel.
    pub fn get_matching_blocks(&self) -> Vec<Match> {
        let (la, lb) = (self.a.len(), self.b.len());
        let mut queue = vec![(0usize, la, 0usize, lb)];
        let mut blocks: Vec<Match> = Vec::new();
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let x = self.find_longest_match(alo, ahi, blo, bhi);
            let (i, j, k) = (x.a, x.b, x.size);
            if k > 0 {
                blocks.push(x);
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        blocks.sort();
        let (mut i1, mut j1, mut k1) = (0, 0, 0);
        let mut out = Vec::new();
        for m in blocks {
            if i1 + k1 == m.a && j1 + k1 == m.b {
                k1 += m.size;
            } else {
                if k1 > 0 {
                    out.push(Match {
                        a: i1,
                        b: j1,
                        size: k1,
                    });
                }
                i1 = m.a;
                j1 = m.b;
                k1 = m.size;
            }
        }
        if k1 > 0 {
            out.push(Match {
                a: i1,
                b: j1,
                size: k1,
            });
        }
        out.push(Match {
            a: la,
            b: lb,
            size: 0,
        });
        out
    }

    fn calc(&self, matches: usize) -> f64 {
        let length = self.a.len() + self.b.len();
        if length > 0 {
            2.0 * matches as f64 / length as f64
        } else {
            1.0
        }
    }

    pub fn ratio(&self) -> f64 {
        let matches: usize = self.get_matching_blocks().iter().map(|m| m.size).sum();
        self.calc(matches)
    }

    pub fn quick_ratio(&self) -> f64 {
        let mut fullbcount: HashMap<char, i64> = HashMap::new();
        for &c in &self.b {
            *fullbcount.entry(c).or_insert(0) += 1;
        }
        let mut avail: HashMap<char, i64> = HashMap::new();
        let mut matches = 0usize;
        for &c in &self.a {
            let numb = match avail.get(&c) {
                Some(&v) => v,
                None => fullbcount.get(&c).copied().unwrap_or(0),
            };
            avail.insert(c, numb - 1);
            if numb > 0 {
                matches += 1;
            }
        }
        self.calc(matches)
    }

    pub fn real_quick_ratio(&self) -> f64 {
        self.calc(self.a.len().min(self.b.len()))
    }
}

/// `difflib.SequenceMatcher(None, a, b).ratio()`.
pub fn ratio(a: &str, b: &str) -> f64 {
    SequenceMatcher::new(a, b).ratio()
}

/// `(Decimal(str(1 - ratio)) * 100).quantize(Decimal("0.01"))`: nary executor / autonomy edit share in %.
pub fn edit_pct_from_ratio(similarity: f64) -> Decimal {
    // str(float) has the same shortest digits Rust prints; Rust's form never uses an exponent.
    let x = 1.0 - similarity;
    let d = Decimal::from_str(&format!("{x}"))
        .or_else(|_| Decimal::from_scientific(&format!("{x:e}")))
        .unwrap_or(Decimal::ZERO);
    let mut q =
        (d * Decimal::from(100)).round_dp_with_strategy(2, RoundingStrategy::MidpointNearestEven);
    q.rescale(2);
    q
}

/// Edit share in % between a draft and what was actually sent (executor.py `compare`).
pub fn edit_pct(a: &str, b: &str) -> Decimal {
    edit_pct_from_ratio(ratio(a, b))
}
