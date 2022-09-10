use redis::Script;

// takes two keys: 
// - bot id
// - bucket id
// 
// returns a tuple containing pass results for each ratelimit
// - (global, bucket)
//
// true means 
pub fn check_global_and_bucket_ratelimit_script() -> Script {
  Script::new(r"
    local global_rl_key = KEYS[1]
    local global_rl_count_key = global_rl_key .. ':count'
    
    local global_rl_key_curr = redis.call('GET', global_rl_key)

    if global_rl_key_curr == false then
      return {false, false}
    end

    local global_rl_limit = tonumber(global_rl_key_curr)

    local global_rl_count_s = redis.call('INCR', global_rl_count_key)
    local global_rl_count = tonumber(global_rl_count_s)

    if global_rl_count >= global_rl_limit then
      return {0, false}
    end

    local bucket_rl_key = KEYS[2]
    local bucket_rl_count_key = bucket_rl_key .. ':count'
    
    local bucket_rl_key_curr = redis.call('GET', bucket_rl_key)

    if bucket_rl_key_curr == false then
      return {global_rl_count, false}
    end

    local bucket_rl_limit = tonumber(bucket_rl_key_curr)

    if bucket_rl_limit == 0 then
      return {global_rl_count, 2}
    end

    local bucket_rl_count_s = redis.call('INCR', bucket_rl_count_key)
    local bucket_rl_count = tonumber(bucket_rl_count_s)

    -- Bucket expiry will have to be set in another request, after the first request has been sent

    if bucket_rl_count >= bucket_rl_limit then
      local old_global_rl_count = redis.call('DECR', global_rl_count_key)
      if tonumber(old_global_rl_count) == 0 then
        redis.call('DEL', global_rl_count_key)
      end

      return {1, 0}
    end

    return {global_rl_count, bucket_rl_count}
  ")
}

// takes two arguments: 
pub fn check_bucket_ratelimit_script() -> Script {
  Script::new(r"
    local rl_key = KEYS[1]
    local rl_count_key = rl_key .. ':count'
    
    local rl_res = redis.call('MGET', rl_key, rl_count_key)

    if rl_res[1] == false then
      return nil
    end

    local rl = tonumber(rl_res[1])
    local rl_count = tonumber(rl_res[2])

    if rl_count == nil then
      rl_count = 0
    end

    if rl_count >= rl then
      return 0
    end

    local count = redis.call('INCR', rl_count_key)

    if count == 1 then
      redis.call('EXPIRE', rl_count_key, 1)
    end

    return count
  ")
}

pub fn lock_ratelimit_script() -> Script { 
  Script::new(r"
    local rl_key = KEYS[1]
    local rl = redis.call('GET', rl_key)

    if rl == false then
      local lock_val = ARGV[1]
      local lock = redis.call('SET', rl_key .. ':lock', lock_val, 'NX', 'EX', '5')

      local got_lock = lock ~= false

      return got_lock
    end

    return false
  ")
}

pub fn unlock_ratelimit_script() -> Script {
  Script::new(r"
    local rl_key = KEYS[1]
    local rl_lock_key = rl_key .. ':lock'

    local lock_val = ARGV[1]
    local rl_lock = redis.call('GET', rl_lock_key)

    if rl_lock == lock_val then
      local rl_limit = ARGV[2]
      redis.call('SET', rl_key, rl_limit)

      local rl_expire_at = tonumber(ARGV[3])
      if rl_expire_at ~= 0 then
        local rl_count_key = rl_key .. ':count'

        redis.call('PEXPIREAT', rl_count_key, rl_expire_at)
      end

      local global_rl_expire_at = tonumber(ARGV[4])
      if global_rl_expire_at ~= nil then
        local global_rl_count_key = KEYS[2] .. ':count'

        redis.call('PEXPIREAT', global_rl_count_key, global_rl_expire_at, 'NX')
      end

      redis.call('DEL', rl_lock_key)
      return true
    end

    return false
  ")
}