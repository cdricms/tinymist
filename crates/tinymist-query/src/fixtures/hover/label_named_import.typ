/// path: acteurs.typ
#let f(..args) = none
#let adherent(..args) = f(name: <adherent>, ..args)
-----
/// compile: true

#import "acteurs.typ": f, adherent
#f(
  adherent(),
  f(/* position after */ <adherent>),
)
