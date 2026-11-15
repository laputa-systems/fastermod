defmodule A do
  def renamed(x) do
    y = x + 1
    y
  end
  def main do
    count = renamed(1)
    if count > 0 do
      helper = count
      IO.inspect(helper)
    end
    IO.inspect(count)
  end
end
