CREATE TABLE verified_checkpoints (
  run_id TEXT PRIMARY KEY REFERENCES solve_runs(id) ON DELETE CASCADE,
  solution_id TEXT NOT NULL UNIQUE,
  checkpoint_json TEXT NOT NULL CHECK (length(checkpoint_json) > 0)
) STRICT;
