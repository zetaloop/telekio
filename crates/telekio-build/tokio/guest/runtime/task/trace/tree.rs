use super::*;

pub(crate) fn format<T>(traces: Vec<Vec<T>>, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
where
    T: Clone + Eq + Hash + fmt::Display,
{
    let mut roots = HashSet::new();
    let mut edges = HashMap::new();
    for trace in traces {
        if let Some(first) = trace.first() {
            roots.insert(first.clone());
        }
        let mut trace = trace.into_iter().peekable();
        while let Some(frame) = trace.next() {
            let subframes = edges.entry(frame).or_insert_with(HashSet::new);
            if let Some(subframe) = trace.peek() {
                subframes.insert(subframe.clone());
            }
        }
    }
    for root in &roots {
        display(formatter, &edges, root, true, " ")?;
    }
    Ok(())
}

fn display<T>(
    formatter: &mut fmt::Formatter<'_>,
    edges: &HashMap<T, HashSet<T>>,
    root: &T,
    last: bool,
    prefix: &str,
) -> fmt::Result
where
    T: Eq + Hash + fmt::Display,
{
    let (current, next) = if last {
        (
            format!("{prefix}└╼\u{a0}{root}"),
            format!("{prefix}\u{a0}\u{a0}\u{a0}"),
        )
    } else {
        (
            format!("{prefix}├╼\u{a0}{root}"),
            format!("{prefix}│\u{a0}\u{a0}"),
        )
    };
    let mut current = current.chars();
    current.next();
    current.next();
    formatter.write_str(current.as_str())?;
    if let Some(children) = edges.get(root) {
        let len = children.len();
        for (index, child) in children.iter().enumerate() {
            writeln!(formatter)?;
            display(formatter, edges, child, index == len - 1, &next)?;
        }
    }
    Ok(())
}
