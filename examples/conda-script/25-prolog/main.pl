% /// conda-script
% channels = ["conda-forge"]
% entrypoint = "swipl ${SCRIPT}"
%
% [dependencies]
% swi-prolog = "*"
% /// end-conda-script
:- initialization(main, main).

:- if(\+ exists_source(library(list_util))).
:- pack_install(list_util, [interactive(false)]).
:- endif.

:- use_module(library(list_util)).

main :-
    numlist(1, 12, Numbers),
    take(5, Numbers, Front),
    take_while([N]>>(N < 4), Numbers, Small),
    format("front: ~w~n", [Front]),
    format("small: ~w~n", [Small]).
