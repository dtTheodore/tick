-- Batched Walrus proofs: a settlement's proof now lives at index `proof_index`
-- inside the BatchProofBlob stored under `walrus_blob_id` (one blob per flush,
-- not per settlement). NULL until the proof flusher publishes the row's batch.
ALTER TABLE settlements ADD COLUMN proof_index INT;
