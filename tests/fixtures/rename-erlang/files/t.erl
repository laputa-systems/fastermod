-module(t).
-export([helper/1]).

helper(X) ->
  case X of
    {ok, V} -> V + bump;
    _ -> bump()
  end.

helper(X, Y) -> X + Y.

main() ->
  Y = helper(1),
  Z = Y + 2,
  io:format("~p~n", [Z]).
