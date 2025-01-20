-- Remove foreign keys from node_subscriptions
ALTER TABLE node_subscriptions 
    DROP CONSTRAINT node_subscriptions_task_small_id_fkey,
    DROP CONSTRAINT node_subscriptions_node_small_id_fkey;

-- Remove foreign keys from refresh_tokens
ALTER TABLE refresh_tokens
    DROP CONSTRAINT refresh_tokens_user_id_fkey;

-- Remove foreign keys from api_tokens
ALTER TABLE api_tokens
    DROP CONSTRAINT api_tokens_user_id_fkey;

-- Remove foreign keys from stacks
ALTER TABLE stacks
    DROP CONSTRAINT stacks_task_small_id_fkey,
    DROP CONSTRAINT stacks_user_id_fkey,
    DROP CONSTRAINT stacks_selected_node_id_task_small_id_fkey;

-- Remove foreign keys from stack_settlement_tickets
ALTER TABLE stack_settlement_tickets
    DROP CONSTRAINT stack_settlement_tickets_stack_small_id_fkey;

-- Remove foreign keys from stack_attestation_disputes
ALTER TABLE stack_attestation_disputes
    DROP CONSTRAINT stack_attestation_disputes_stack_small_id_fkey;

-- Remove foreign keys from node performance tables
ALTER TABLE node_throughput_performance
    DROP CONSTRAINT node_throughput_performance_node_small_id_fkey;

ALTER TABLE node_prefill_performance
    DROP CONSTRAINT node_prefill_performance_node_small_id_fkey;

ALTER TABLE node_decode_performance
    DROP CONSTRAINT node_decode_performance_node_small_id_fkey;

ALTER TABLE node_latency_performance
    DROP CONSTRAINT node_latency_performance_node_small_id_fkey;

-- Remove foreign keys from node_public_keys
ALTER TABLE node_public_keys
    DROP CONSTRAINT node_public_keys_node_small_id_fkey;