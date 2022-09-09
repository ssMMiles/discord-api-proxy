request = function() 
  url_path = "/api/v10/users/@me"

  return wrk.format("GET", url_path)
end