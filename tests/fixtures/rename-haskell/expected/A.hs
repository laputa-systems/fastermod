module A where

renamed :: Int -> Int
renamed x = x + 1

main :: IO ()
main = do
  let count = renamed 1
  let helper = count
  print helper
  print count
