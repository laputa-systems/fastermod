defmodule A do
  def renamed(x), do: x + 1

  def main do
    count = renamed(1)
    # helper in comment
    s = "helper"
    IO.inspect(count)
    IO.inspect(s)
  end
end
