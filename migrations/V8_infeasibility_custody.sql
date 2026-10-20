ALTER TABLE solve_runs
ADD COLUMN source_asset_bindings_json TEXT;

ALTER TABLE counterfactual_jobs
ADD COLUMN terminal_authority_json TEXT;

CREATE TABLE run_infeasibility_evidence (
  run_id TEXT PRIMARY KEY REFERENCES solve_runs(id) ON DELETE CASCADE,
  evidence_json TEXT NOT NULL CHECK (
    length(CAST(evidence_json AS BLOB)) > 0
    AND length(CAST(evidence_json AS BLOB)) <= 1048576
  )
) STRICT;

CREATE TRIGGER solve_runs_validate_source_asset_bindings_insert
BEFORE INSERT ON solve_runs
WHEN NEW.source_asset_bindings_json IS NOT NULL
  AND length(CAST(NEW.source_asset_bindings_json AS BLOB)) > 2097152
BEGIN
  SELECT RAISE(ABORT, 'source asset bindings exceed byte limit');
END;

CREATE TRIGGER solve_runs_validate_source_asset_bindings_update
BEFORE UPDATE OF source_asset_bindings_json ON solve_runs
WHEN NEW.source_asset_bindings_json IS NOT NULL
  AND length(CAST(NEW.source_asset_bindings_json AS BLOB)) > 2097152
BEGIN
  SELECT RAISE(ABORT, 'source asset bindings exceed byte limit');
END;

CREATE TRIGGER counterfactual_jobs_validate_terminal_authority_insert
BEFORE INSERT ON counterfactual_jobs
WHEN NEW.terminal_authority_json IS NOT NULL
  AND length(CAST(NEW.terminal_authority_json AS BLOB)) > 1048576
BEGIN
  SELECT RAISE(ABORT, 'terminal authority exceeds byte limit');
END;

CREATE TRIGGER counterfactual_jobs_validate_terminal_authority_update
BEFORE UPDATE OF terminal_authority_json ON counterfactual_jobs
WHEN NEW.terminal_authority_json IS NOT NULL
  AND length(CAST(NEW.terminal_authority_json AS BLOB)) > 1048576
BEGIN
  SELECT RAISE(ABORT, 'terminal authority exceeds byte limit');
END;
