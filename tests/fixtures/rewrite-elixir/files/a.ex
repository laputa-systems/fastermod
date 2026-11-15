defmodule A do
  def helper(x), do: x

  def main do
    a = helper(1, 2)
    b = helper(x)
    c = other(1)
    # helper(9) in comment
    IO.inspect(a + b + c)
  end
end
