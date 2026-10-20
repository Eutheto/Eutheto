-- V5 added private ownership columns but historical completed jobs have NULLs.
-- Restore their exact derived run/request identity from their checksummed result.
-- Existing V5 ownership triggers and unique indexes reject missing or conflicting run authority.
UPDATE counterfactual_jobs
SET derived_run_id = json_extract(result_json, '$.runInput.runId'),
    derived_request_id = json_extract(result_json, '$.runInput.requestId')
WHERE state = 'completed'
  AND derived_run_id IS NULL
  AND derived_request_id IS NULL;
