local function helper(x)
  return x
end

local a = renamed(1, 2)
local b = renamed(x)
local c = other(1)
-- helper(9) in comment
print(a)
