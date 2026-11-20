/// compile: true

#set heading(numbering: "1.")
= Titre <titre>

#let f(..args) = none
#f(
  f(name: <portique>),
  f(name: <isole>),
  f(<portique>, <supprime>),
  f(<titre>),
)
