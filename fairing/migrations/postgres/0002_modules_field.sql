ALTER TABLE deployments
ADD modules JSONB NOT NULL DEFAULT '[]';
