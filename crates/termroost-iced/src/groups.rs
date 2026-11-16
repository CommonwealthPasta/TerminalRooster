//! The projects list with groups: the rows it shows, and what moving a row
//! (by dragging, or a project with the keyboard) does.

use termroost_core::{Command, GroupId, Project, ProjectId};

use crate::reorder::Target;

/// A line of the projects list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// A group, above its projects.
    Group(GroupId),
    Project(ProjectId),
}

/// A project as the list sees it: its id and its group.
pub type Entry = (ProjectId, Option<GroupId>);

pub fn entries<'a>(projects: impl IntoIterator<Item = &'a Project>) -> Vec<Entry> {
    projects.into_iter().map(|p| (p.id, p.group)).collect()
}

/// The rows for the `shown` projects (in list order), with their depths: each
/// group's row comes before its projects, which are left out while `open`
/// says the group is closed.
pub fn rows(shown: &[Entry], open: impl Fn(GroupId) -> bool) -> Vec<(Row, u8)> {
    let mut rows = Vec::new();
    let mut current = None;
    for &(project, group) in shown {
        if let Some(g) = group
            && group != current
        {
            rows.push((Row::Group(g), 0));
        }
        current = group;
        match group {
            None => rows.push((Row::Project(project), 0)),
            Some(g) if open(g) => rows.push((Row::Project(project), 1)),
            Some(_) => {}
        }
    }
    rows
}

fn index(list: &[Entry], project: ProjectId) -> Option<usize> {
    list.iter().position(|e| e.0 == project)
}

fn group_of(list: &[Entry], project: ProjectId) -> Option<GroupId> {
    list.iter().find(|e| e.0 == project).and_then(|e| e.1)
}

/// `project` moved to `to_index` of `all` without it, into `group`; `None`
/// if that is where it is.
fn move_project(all: &[Entry], project: ProjectId, to_index: usize, group: Option<GroupId>) -> Option<Command> {
    let unchanged = index(all, project) == Some(to_index) && group_of(all, project) == group;
    (!unchanged).then_some(Command::MoveProject { project, to_index, group })
}

/// What dropping `rows[from]` on `target` does, `all` being every project in
/// list order (the rows may show only some); `None` if nothing changes or it
/// can't go there.
pub fn drop_command(all: &[Entry], rows: &[Row], from: usize, target: Target) -> Option<Command> {
    match (*rows.get(from)?, target) {
        (Row::Project(p), Target::Onto(i)) => match *rows.get(i)? {
            // Onto a project of its own group, it is already with it.
            Row::Project(q) if q != p && (group_of(all, p).is_none() || group_of(all, p) != group_of(all, q)) => {
                Some(Command::GroupProjects { project: p, with: q })
            }
            Row::Group(g) if group_of(all, p) != Some(g) => {
                let rest: Vec<Entry> = all.iter().copied().filter(|e| e.0 != p).collect();
                let last = rest.iter().rposition(|e| e.1 == Some(g))?;
                move_project(all, p, last + 1, Some(g))
            }
            _ => None,
        },
        (Row::Project(p), Target::Between { above, below, depth }) => {
            let rest: Vec<Entry> = all.iter().copied().filter(|e| e.0 != p).collect();
            if depth > 0 {
                // In a group: just above the project below, or just below the one above.
                if let Some(Row::Project(q)) = row_at(rows, below)
                    && let Some(g) = group_of(all, q)
                {
                    return move_project(all, p, index(&rest, q)?, Some(g));
                }
                let Some(Row::Project(q)) = row_at(rows, above) else { return None };
                let g = group_of(all, q)?;
                move_project(all, p, index(&rest, q)? + 1, Some(g))
            } else {
                let to_index = match row_at(rows, below) {
                    None => rest.len(),
                    Some(Row::Group(h)) => rest.iter().position(|e| e.1 == Some(h))?,
                    Some(Row::Project(q)) if group_of(all, q).is_none() => index(&rest, q)?,
                    Some(Row::Project(_)) => return None,
                };
                move_project(all, p, to_index, None)
            }
        }
        // A group goes between other groups and projects, never into a group.
        (Row::Group(g), Target::Between { below, depth: 0, .. }) => {
            let rest: Vec<Entry> = all.iter().copied().filter(|e| e.1 != Some(g)).collect();
            let to_index = match row_at(rows, below) {
                None => rest.len(),
                Some(Row::Group(h)) => rest.iter().position(|e| e.1 == Some(h))?,
                Some(Row::Project(q)) if group_of(all, q).is_none() => index(&rest, q)?,
                Some(Row::Project(_)) => return None,
            };
            let now = all.iter().position(|e| e.1 == Some(g))?;
            (to_index != now).then_some(Command::MoveGroup { group: g, to_index })
        }
        (Row::Group(_), _) => None,
    }
}

fn row_at(rows: &[Row], i: Option<usize>) -> Option<Row> {
    i.and_then(|i| rows.get(i).copied())
}

/// Moving `project` one place down (or up) with the keyboard, among the
/// `shown` projects: past the next one in its group, out of its group at the
/// group's end, or past the whole next group. `None` at the end of the list.
pub fn step(all: &[Entry], shown: &[Entry], project: ProjectId, down: bool) -> Option<Command> {
    let i = index(shown, project)?;
    let next = if down { shown.get(i + 1) } else { i.checked_sub(1).and_then(|i| shown.get(i)) };
    let rest: Vec<Entry> = all.iter().copied().filter(|e| e.0 != project).collect();
    match group_of(all, project) {
        Some(g) => match next.filter(|e| e.1 == Some(g)) {
            Some(&(q, _)) => move_project(all, project, index(&rest, q)? + usize::from(down), Some(g)),
            // Out of the group, just past its end.
            None => {
                let edge = if down {
                    rest.iter().rposition(|e| e.1 == Some(g)).map(|i| i + 1)
                } else {
                    rest.iter().position(|e| e.1 == Some(g))
                };
                move_project(all, project, edge.or(index(all, project))?, None)
            }
        },
        None => {
            let &(q, h) = next?;
            let to_index = match h {
                // Past the whole group.
                Some(h) if down => rest.iter().rposition(|e| e.1 == Some(h))? + 1,
                Some(h) => rest.iter().position(|e| e.1 == Some(h))?,
                None => index(&rest, q)? + usize::from(down),
            };
            move_project(all, project, to_index, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: GroupId = GroupId(100);
    const H: GroupId = GroupId(200);

    /// Projects by letter, upper case in group G, digits in group H, the
    /// rest in no group: "aBCd" is a, then B and C in G, then d.
    fn list(s: &str) -> Vec<Entry> {
        s.chars()
            .map(|c| {
                let group = if c.is_ascii_uppercase() {
                    Some(G)
                } else if c.is_ascii_digit() {
                    Some(H)
                } else {
                    None
                };
                (ProjectId(c.to_ascii_lowercase() as i64), group)
            })
            .collect()
    }

    /// `all` after `cmd`, written the way [`list`] reads it.
    fn after(all: &str, cmd: Option<Command>) -> String {
        let all = list(all);
        let Some(cmd) = cmd else { return "unchanged".into() };
        let (mut ids, mut groups): (Vec<ProjectId>, Vec<Option<GroupId>>) = all.iter().copied().unzip();
        let mut take = |p: ProjectId| {
            let i = ids.iter().position(|x| *x == p).unwrap();
            groups.remove(i);
            ids.remove(i)
        };
        match cmd {
            Command::MoveProject { project, to_index, group } => {
                take(project);
                ids.insert(to_index, project);
                groups.insert(to_index, group);
            }
            Command::MoveGroup { group, to_index } => {
                let members: Vec<ProjectId> = all.iter().filter(|e| e.1 == Some(group)).map(|e| e.0).collect();
                for m in &members {
                    take(*m);
                }
                for (k, m) in members.iter().enumerate() {
                    ids.insert(to_index + k, *m);
                    groups.insert(to_index + k, Some(group));
                }
            }
            Command::GroupProjects { project, with } => return format!("group {} with {}", name(project), name(with)),
            other => panic!("unexpected {other:?}"),
        }
        ids.iter()
            .zip(groups)
            .map(|(p, g)| match g {
                Some(G) => name(*p).to_ascii_uppercase(),
                Some(_) => name(*p),
                None => name(*p),
            })
            .collect()
    }

    fn name(p: ProjectId) -> char {
        char::from(p.0 as u8)
    }

    fn rows_of(shown: &str, closed: &[GroupId]) -> Vec<Row> {
        rows(&list(shown), |g| !closed.contains(&g)).into_iter().map(|(r, _)| r).collect()
    }

    fn p(c: char) -> Row {
        Row::Project(ProjectId(c as i64))
    }

    #[test]
    fn groups_head_their_projects_and_closed_ones_hide_them() {
        let shown = list("aBC1d");
        let rows = rows(&shown, |g| g != H);
        assert_eq!(rows, [(p('a'), 0), (Row::Group(G), 0), (p('b'), 1), (p('c'), 1), (Row::Group(H), 0), (p('d'), 0),]);
        assert_eq!(rows_of("", &[]), []);
    }

    #[test]
    fn dropping_a_project_between_rows() {
        // Rows: a, G, B, C, d.
        let rows = rows_of("aBCd", &[]);
        let between = |above, below, depth| Target::Between { above, below, depth };
        let drop = |from, t| after("aBCd", drop_command(&list("aBCd"), &rows, from, t));
        assert_eq!(drop(0, between(Some(2), Some(3), 1)), "BACd", "a into G, between B and C");
        assert_eq!(drop(0, between(Some(3), Some(4), 1)), "BCAd", "a into G, after C");
        assert_eq!(drop(0, between(Some(3), Some(4), 0)), "BCad", "a after G");
        assert_eq!(drop(4, between(Some(1), Some(2), 1)), "aDBC", "d into G, first");
        assert_eq!(drop(4, between(None, Some(0), 0)), "daBC");
        assert_eq!(drop(3, between(Some(4), None, 0)), "aBdc", "C out of G, to the end");
        assert_eq!(drop(3, between(Some(0), Some(1), 0)), "acBd", "C out of G, above it");
        assert_eq!(drop(2, between(Some(3), Some(4), 1)), "aCBd", "B after C");
        assert_eq!(drop(3, between(Some(2), Some(4), 1)), "unchanged", "C where it was");
        assert_eq!(drop(3, between(Some(1), Some(2), 1)), "aCBd", "C before B");
        assert_eq!(drop(0, between(None, Some(1), 0)), "unchanged");
    }

    #[test]
    fn dropping_a_project_onto_a_row() {
        let rows = rows_of("aBCd", &[]);
        let drop = |from, onto| after("aBCd", drop_command(&list("aBCd"), &rows, from, Target::Onto(onto)));
        assert_eq!(drop(0, 4), "group a with d");
        assert_eq!(drop(0, 2), "group a with b", "into b's group");
        assert_eq!(drop(4, 1), "aBCD", "onto a group: its last");
        assert_eq!(drop(2, 3), "unchanged", "already with it");
        assert_eq!(drop(2, 1), "unchanged", "already in it");
        assert_eq!(drop(0, 0), "unchanged");
    }

    #[test]
    fn dropping_a_group() {
        // Rows: a, G, B, C, H, 1, d.
        let rows = rows_of("aBC1d", &[]);
        let between = |above, below, depth| Target::Between { above, below, depth };
        let drop = |from, t| after("aBC1d", drop_command(&list("aBC1d"), &rows, from, t));
        assert_eq!(drop(1, between(None, Some(0), 0)), "BCa1d");
        assert_eq!(drop(1, between(Some(5), Some(6), 0)), "a1BCd", "past H");
        assert_eq!(drop(1, between(Some(6), None, 0)), "a1dBC");
        assert_eq!(drop(4, between(Some(0), Some(1), 0)), "a1BCd", "H above G");
        assert_eq!(drop(1, between(Some(0), Some(4), 0)), "unchanged");
        assert_eq!(drop(1, between(Some(4), Some(5), 1)), "unchanged", "not into a group");
        assert_eq!(drop(1, Target::Onto(0)), "unchanged", "not onto anything");
        // Closed, it is one row: a, G, H, 1, d.
        let rows = rows_of("aBC1d", &[G]);
        let drop = |from, t| after("aBC1d", drop_command(&list("aBC1d"), &rows, from, t));
        assert_eq!(drop(1, between(Some(3), Some(4), 0)), "a1BCd");
    }

    #[test]
    fn drops_in_a_filtered_list_land_among_all_projects() {
        // Searching shows a, C and e of "aBCde": rows a, G, C, e.
        let all = "aBCde";
        let rows = rows_of("aCe", &[]);
        let between = |above, below, depth| Target::Between { above, below, depth };
        let drop = |from, t| after(all, drop_command(&list(all), &rows, from, t));
        assert_eq!(drop(3, between(Some(2), None, 1)), "aBCEd", "just below C, in G");
        assert_eq!(drop(3, between(Some(1), Some(2), 1)), "aBECd", "just above C");
        assert_eq!(drop(0, between(Some(2), Some(3), 0)), "BCdae", "just above e");
        assert_eq!(drop(3, Target::Onto(1)), "aBCEd", "onto G: after all of its projects");
    }

    #[test]
    fn keyboard_moves_go_through_groups_one_project_at_a_time() {
        let step = |all: &str, shown: &str, c: char, down: bool| {
            after(all, step(&list(all), &list(shown), ProjectId(c as i64), down))
        };
        let all = "aBCd";
        assert_eq!(step(all, all, 'a', true), "BCad", "past the whole group");
        assert_eq!(step(all, all, 'd', false), "adBC");
        assert_eq!(step(all, all, 'b', true), "aCBd", "within the group");
        assert_eq!(step(all, all, 'c', true), "aBcd", "out at its end");
        assert_eq!(step(all, all, 'b', false), "abCd", "out at its start");
        assert_eq!(step(all, all, 'a', false), "unchanged", "the top");
        assert_eq!(step(all, all, 'd', true), "unchanged", "the bottom");
        assert_eq!(step("aBd", "aBd", 'b', true), "abd", "its only project: out of it");
        // While searching, past the projects shown.
        assert_eq!(step("aBCDe", "aDe", 'a', true), "BCDae", "the group's hidden projects too");
        assert_eq!(step("aBCDe", "aDe", 'd', false), "adBCe", "out of the group past hidden ones");
    }
}
