ALTER TABLE stacks
ADD COLUMN locked_compute_units BIGINT DEFAULT 0;

ALTER TABLE stacks
DROP CONSTRAINT check_compute_units_constraint;

ALTER TABLE stacks 
    ADD CONSTRAINT check_compute_units_constraint 
    CHECK (already_computed_units + locked_compute_units <= num_compute_units);
