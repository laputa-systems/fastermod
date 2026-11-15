module A where

renamed :: Int -> Int
renamed x = x + 1

main :: IO ()
main = do
  let count = renamed 1
  -- helper in comment
  let s = "helper"
  print count
  putStrLn s
