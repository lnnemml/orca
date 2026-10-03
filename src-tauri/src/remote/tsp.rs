//! Matching a raw `tsp -l` row to a job (ADR-024 l, probe P4).
//!
//! Recorded `tsp -l` (tsp 1.0.1): a header, then one row per task — `ID State Output E-Level
//! Times Command` — where the command is the task's argv **joined by single spaces, unquoted**,
//! and the Output / E-Level / Times columns may be blank. So a row belongs to a job only if the
//! job dir appears as a **whole space-separated token**; a substring match would let `/jobs/j1`
//! claim the row of `/jobs/j10`. Rows are never matched by id: ids are per daemon and restart at
//! 0 after a daemon restart (P4).

use super::FactError;

/// A task state as `tsp -l` prints it. Only the three recorded states are known; any other word
/// in a row that belongs to the job is a parse error, not a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TspState {
    Queued,
    Running,
    Finished,
}

/// The parts of a matched row the classifier uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TspRow {
    pub id: u64,
    pub state: TspState,
}

/// Does this raw row belong to `job_dir`? `Ok(None)` if the job dir is not one of its tokens
/// (another job's row, or the header). If it is, the row's id and state must parse; otherwise
/// this is an error, because the row is ours but unreadable.
pub fn match_job_row(row: &str, job_dir: &str) -> Result<Option<TspRow>, FactError> {
    if !row_mentions_job_dir(row, job_dir) {
        return Ok(None);
    }
    let mut tokens = row.split_whitespace();
    let id_text = tokens.next().unwrap_or_default();
    let state_text = tokens.next().unwrap_or_default();

    if id_text.is_empty() || !id_text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(FactError::TspRow(format!("id is not a number: {row:?}")));
    }
    let id = id_text
        .parse::<u64>()
        .map_err(|e| FactError::TspRow(format!("id {id_text:?}: {e}")))?;
    let state = match state_text {
        "queued" => TspState::Queued,
        "running" => TspState::Running,
        "finished" => TspState::Finished,
        other => return Err(FactError::TspRow(format!("unknown state {other:?}: {row:?}"))),
    };
    Ok(Some(TspRow { id, state }))
}

/// The job dir is one whole whitespace-separated token of the row.
fn row_mentions_job_dir(row: &str, job_dir: &str) -> bool {
    row.split_whitespace().any(|token| token == job_dir)
}

/// The probe P4 `tsp -l` output, verbatim, shared with the classifier's table tests.
#[cfg(test)]
pub(crate) mod fixtures {
    pub const TSP_L_P4: &str = "\
ID   State      Output               E-Level  Times(r/u/s)   Command [run=1/1]
3    running    /tmp/ts-out.S0SWpm                           bash -c sleep 100 /home/anton/.orcastudio/probe-5.2/jobs/job-with-a-very-long-name-0123456789-0123456789-0123456789-0123456789-0123456789-0123456789-0123456789-0123456789-0123456789 4-7 /opt/orca
4    queued     (file)                                       bash -c sleep 100 /home/anton/.orcastudio/probe-5.2/jobs/j_queued 8-11 /opt/orca
0    finished   /tmp/ts-out.QG5j6G   0        0.00/0.00/0.00 bash -c exit 0 /home/anton/.orcastudio/probe-5.2/jobs/j_ok 0-3 /opt/orca
1    finished   /tmp/ts-out.NlzNMk   3        0.00/0.00/0.00 bash -c exit 3 /home/anton/.orcastudio/probe-5.2/jobs/j_fail 0-3 /opt/orca
2    finished   /tmp/ts-out.JzAIe7   -1       1.00/0.00/0.00 bash -c sleep 100 /home/anton/.orcastudio/probe-5.2/jobs/j_killed 0-3 /opt/orca
";

    pub const JOBS: &str = "/home/anton/.orcastudio/probe-5.2/jobs";

    /// The P4 row with `id` (the first token), verbatim.
    pub fn p4_row(id: &str) -> String {
        TSP_L_P4
            .lines()
            .find(|l| l.split_whitespace().next() == Some(id))
            .map(str::to_string)
            .unwrap_or_default()
    }

    /// A row derived from the recorded queued row (id 4) with its job dir replaced — synthetic,
    /// the recorded shape with a different dir.
    pub fn queued_row_for(job_dir: &str) -> String {
        p4_row("4").replace(&format!("{JOBS}/j_queued"), job_dir)
    }

    /// As [`queued_row_for`], from the recorded running row (id 3).
    pub fn running_row_for(job_dir: &str) -> String {
        let long = format!(
            "{JOBS}/job-with-a-very-long-name-0123456789-0123456789-0123456789-0123456789-0123456789-0123456789-0123456789-0123456789-0123456789"
        );
        p4_row("3").replace(&long, job_dir)
    }

    /// As [`queued_row_for`], from the recorded finished row (id 0, `exit 0`).
    pub fn finished_row_for(job_dir: &str) -> String {
        p4_row("0").replace(&format!("{JOBS}/j_ok"), job_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    fn find(job_dir: &str) -> Vec<TspRow> {
        TSP_L_P4
            .lines()
            .filter_map(|row| match_job_row(row, job_dir).unwrap())
            .collect()
    }

    #[test]
    fn recorded_rows_match_by_job_dir() {
        assert_eq!(find(&format!("{JOBS}/j_queued")), vec![TspRow { id: 4, state: TspState::Queued }]);
        assert_eq!(find(&format!("{JOBS}/j_ok")), vec![TspRow { id: 0, state: TspState::Finished }]);
        assert_eq!(find(&format!("{JOBS}/j_killed")), vec![TspRow { id: 2, state: TspState::Finished }]);
        let long = format!(
            "{JOBS}/job-with-a-very-long-name-0123456789-0123456789-0123456789-0123456789-0123456789-0123456789-0123456789-0123456789-0123456789"
        );
        assert_eq!(find(&long), vec![TspRow { id: 3, state: TspState::Running }]);
        assert!(find(&format!("{JOBS}/j_absent")).is_empty());
    }

    #[test]
    fn header_is_not_a_row() {
        let header = TSP_L_P4.lines().next().unwrap();
        assert_eq!(match_job_row(header, &format!("{JOBS}/j_ok")).unwrap(), None);
    }

    #[test]
    fn job_dir_is_a_whole_token_not_a_substring() {
        // `/jobs/j1` must not claim the row of `/jobs/j10`, nor `/jobs/j_ok` the row of a
        // longer sibling.
        let j10 = queued_row_for(&format!("{JOBS}/j10"));
        assert_eq!(match_job_row(&j10, &format!("{JOBS}/j1")).unwrap(), None);
        assert_eq!(
            match_job_row(&j10, &format!("{JOBS}/j10")).unwrap(),
            Some(TspRow { id: 4, state: TspState::Queued })
        );
        // Nor a parent dir.
        assert_eq!(match_job_row(&j10, JOBS).unwrap(), None);
    }

    #[test]
    fn our_row_with_an_unknown_state_is_an_error() {
        let dir = format!("{JOBS}/j_queued");
        let row = p4_row("4");
        assert!(match_job_row(&row.replace("queued     (file)", "allocating (file)"), &dir).is_err());
        assert!(match_job_row(&row.replacen("4 ", "x ", 1), &dir).is_err());
        assert!(match_job_row(&dir, &dir).is_err(), "a bare dir is not a row");
    }
}
