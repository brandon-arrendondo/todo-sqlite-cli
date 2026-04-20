use serde::Serialize;
use serde_json::json;

use crate::db::Task;

pub fn print_task_json(task: &Task) {
    println!("{}", serde_json::to_string(task).unwrap());
}

pub fn print_tasks_json(tasks: &[Task]) {
    let v = json!({ "tasks": tasks });
    println!("{}", serde_json::to_string(&v).unwrap());
}

#[derive(Serialize)]
struct DateGroup<'a> {
    date: String,
    tasks: Vec<&'a Task>,
}

pub fn print_completed_json(tasks: &[Task]) {
    let mut groups: std::collections::BTreeMap<String, Vec<&Task>> =
        std::collections::BTreeMap::new();
    for t in tasks {
        let date = t
            .completed_at
            .as_deref()
            .map(|s| s.split('T').next().unwrap_or(s).to_string())
            .unwrap_or_default();
        groups.entry(date).or_default().push(t);
    }
    let mut out: Vec<DateGroup> = groups
        .into_iter()
        .map(|(date, tasks)| DateGroup { date, tasks })
        .collect();
    out.sort_by(|a, b| b.date.cmp(&a.date));
    let v = json!({ "completed": out });
    println!("{}", serde_json::to_string_pretty(&v).unwrap());
}

pub fn print_task_text(task: &Task) {
    println!("# Task ID: {}", task.id);
    println!("# Title: {}", task.title);
    println!("# Status: {}", task.status);
    println!("# Priority: P{}", task.priority);
    if !task.depends_on.is_empty() {
        let deps: Vec<String> = task.depends_on.iter().map(|d| d.to_string()).collect();
        let suffix = if task.blocked { " (blocked)" } else { "" };
        println!("# Dependencies: {}{}", deps.join(", "), suffix);
    }
    if !task.tags.is_empty() {
        println!("# Tags: {}", task.tags.join(", "));
    }
    println!("# Created: {}", task.created_at);
    if let Some(s) = &task.started_at {
        println!("# Started: {s}");
    }
    if let Some(c) = &task.completed_at {
        println!("# Completed: {c}");
    }
    if let Some(d) = &task.details {
        println!("# Details:");
        println!("{d}");
    }
}

pub fn print_tasks_table(tasks: &[Task]) {
    if tasks.is_empty() {
        return;
    }
    for t in tasks {
        let blocked = if t.blocked { " [blocked]" } else { "" };
        let tags = if t.tags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", t.tags.join(","))
        };
        println!(
            "{:>4}  {:<11}  P{}  {}{}{}",
            t.id, t.status, t.priority, t.title, tags, blocked
        );
    }
}

pub fn markdown_todo(tasks: &[Task]) -> String {
    let mut buf = String::new();
    buf.push_str("# TODO\n\n");
    for t in tasks {
        buf.push_str(&format!("# Task ID: {}\n", t.id));
        buf.push_str(&format!("# Title: {}\n", t.title));
        buf.push_str(&format!("# Status: {}\n", t.status));
        buf.push_str(&format!("# Priority: P{}\n", t.priority));
        if !t.depends_on.is_empty() {
            let deps: Vec<String> = t.depends_on.iter().map(|d| d.to_string()).collect();
            let suffix = if t.blocked { " (blocked)" } else { "" };
            buf.push_str(&format!("# Dependencies: {}{}\n", deps.join(", "), suffix));
        } else {
            buf.push_str("# Dependencies: none\n");
        }
        if !t.tags.is_empty() {
            buf.push_str(&format!("# Tags: {}\n", t.tags.join(", ")));
        }
        if let Some(d) = &t.details {
            buf.push_str("# Details:\n");
            buf.push_str(d);
            if !d.ends_with('\n') {
                buf.push('\n');
            }
        }
        buf.push_str("\n---\n\n");
    }
    buf
}
