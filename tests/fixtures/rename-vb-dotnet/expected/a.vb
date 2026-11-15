Module M
    Function Renamed(x As Integer) As Integer
        Return Bump(x)
    End Function
    Sub Main()
        Dim y = Renamed(1)
        Dim helper = 5
        Console.WriteLine(helper)
    End Sub
End Module
