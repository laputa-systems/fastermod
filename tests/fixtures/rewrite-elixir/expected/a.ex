defmodule A do
  def renamed(x), do: x

  def main do
    a = renamed(1, 2)
    b = renamed(x)
    c = other(1)
    # helper(9) in comment
    IO.inspect(a + b + c)
  end
end
