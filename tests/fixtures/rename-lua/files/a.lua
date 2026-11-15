local function helper(x)
  local y = x + 1
  return y
end

local count = helper(1)
local helper = 5
print(helper, count)
for i = 1, 3 do print(i) end
