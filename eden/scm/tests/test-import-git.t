#if py2
#else
Python 3 email parser is slightly different - it inserts a space in "user".
  $ hg --encoding utf-8 log -r .
  changeset:   *:* (glob)
  user:        Rapha\xc3\xabl Hertzog  <hertzog@debian.org> (esc)
  date:        * (glob)
  summary:     \xc5\xa7\xe2\x82\xac\xc3\x9f\xe1\xb9\xaa (esc)
  
#endif