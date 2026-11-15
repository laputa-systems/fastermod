-module(t).
-export([renamed/1]).

renamed(X) ->
  case X of
    {ok, V} -> V + bump;
    _ -> bump()
  end.

helper(X, Y) -> X + Y.

main() ->
  Y = renamed(1),
  Z = Y + 2,
  io:format("~p~n", [Z]).
