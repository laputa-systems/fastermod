module A where

helper :: Int -> Int
helper x = x + 1

main :: IO ()
main = do
  let count = helper 1
  -- helper in comment
  let s = "helper"
  print count
  putStrLn s
