module A where

helper :: Int -> Int -> Int
helper x y = x + y

main :: IO ()
main = do
  let a = renamed 1 2
  let b = renamed x y
  let c = other 1
  print a
