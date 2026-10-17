//! Which candidates two published "did you mean" rules would offer for
//! mistyped filter fields and values.

const FIELDS: &[&str] = &[
    "type",
    "title",
    "status",
    "assignee",
    "reporter",
    "agent",
    "parent",
    "label",
    "priority",
    "created",
    "updated",
    "area",
    "due",
    "estimate",
    "iteration",
    "sort",
    "key",
    "project",
];
const STATUSES: &[&str] = &[
    "triage",
    "todo",
    "in_progress",
    "review",
    "testing",
    "done",
    "cancelled",
];

/// clap_builder's `did_you_mean`: Jaro similarity above 0.7, best first.
fn clap_rule<'a>(input: &str, options: &[&'a str]) -> Vec<(&'a str, f64)> {
    let mut out: Vec<_> = options
        .iter()
        .map(|o| (*o, strsim::jaro(input, o)))
        .filter(|(_, c)| *c > 0.7)
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

/// rustc's `find_best_match_for_name`: case-insensitive exact match first,
/// else the closest within max(len, 3) / 3 edits.
fn rustc_rule<'a>(input: &str, options: &[&'a str]) -> Option<(&'a str, usize)> {
    if let Some(o) = options.iter().find(|o| o.eq_ignore_ascii_case(input)) {
        return Some((o, 0));
    }
    let limit = input.chars().count().max(3) / 3;
    options
        .iter()
        .map(|o| (*o, strsim::levenshtein(input, o)))
        .filter(|(_, d)| *d <= limit)
        .min_by_key(|(_, d)| *d)
}

fn main() {
    let cases: &[(&str, &[&str])] = &[
        ("statu", FIELDS),
        ("stauts", FIELDS),
        ("asignee", FIELDS),
        ("labels", FIELDS),
        ("prio", FIELDS),
        ("Status", FIELDS),
        ("tpye", FIELDS),
        ("xyz", FIELDS),
        ("opn", STATUSES),
        ("reveiw", STATUSES),
        ("in-progress", STATUSES),
        ("inprogress", STATUSES),
        ("test", STATUSES),
        ("Done", STATUSES),
    ];
    for (input, options) in cases {
        let clap: Vec<String> = clap_rule(input, options)
            .iter()
            .map(|(o, c)| format!("{o} {c:.2}"))
            .collect();
        let rustc = rustc_rule(input, options).map(|(o, d)| format!("{o} (d={d})"));
        println!(
            "{input:<12} clap(jaro>0.7): [{}]  rustc(lev<=len/3): {}",
            clap.join(", "),
            rustc.unwrap_or("-".into())
        );
    }
}
