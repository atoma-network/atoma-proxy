ALTER TABLE stacks 
    ADD CONSTRAINT check_compute_units_constraint 
    CHECK (already_computed_units <= num_compute_units);
