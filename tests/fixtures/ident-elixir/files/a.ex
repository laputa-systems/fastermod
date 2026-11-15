defmodule A do
  def helper(x), do: x + 1

  def main do
    count = helper(1)
    # helper in comment
    s = "helper"
    IO.inspect(count)
    IO.inspect(s)
  end
end
