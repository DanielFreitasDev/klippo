O **Klipper** é um aplicativo que salva o histórico da área de transferência. Ou seja, tudo o que eu copio, seja texto ou imagem, fica registrado em um histórico. Com o atalho **Super + V**, consigo acessar esse histórico e escolher rapidamente qual item quero reutilizar.

No entanto, o Klipper é nativo dos sistemas com **KDE Plasma**. Por isso, não consigo utilizá-lo facilmente no Ubuntu, por exemplo, a não ser recorrendo a alternativas. Mesmo assim, eu gosto muito do Klipper: a integração com o sistema, o design, a simplicidade e, ao mesmo tempo, o poder das funcionalidades. É exatamente esse equilíbrio que me agrada.

Pensando nisso, gostaria de saber: se você fosse recriar o Klipper, inicialmente com foco em rodar no **Ubuntu**, em distribuições baseadas no Ubuntu, nos flavors do Ubuntu e, de forma geral, em sistemas Linux, qual linguagem de programação você usaria e como desenvolveria esse aplicativo?

O aplicativo deveria funcionar tanto em sessões **Wayland** quanto em **X11**, garantindo compatibilidade com diferentes ambientes gráficos do Linux. A ideia seria oferecer uma experiência consistente independentemente do servidor gráfico utilizado pelo sistema.

O design teria que ser extremamente parecido com o Klipper. O aplicativo deveria ter **modo claro e modo escuro**, uma **caixa de pesquisa**, e listar os itens copiados em **ordem decrescente**, deixando sempre o item mais recente no topo.

A pesquisa também deveria funcionar em tempo real: ao digitar na caixa de busca, os resultados seriam filtrados automaticamente; ao apagar o texto pesquisado, a lista voltaria ao estado original. Além disso, ao pressionar **Super + V**, a janela deveria abrir próxima à posição atual do cursor do mouse.

Também gostaria que a fonte padrão fosse a **JetBrains Mono**. Ao lado de cada item listado, deveria haver uma opção para removê-lo individualmente do histórico. Nas configurações, o usuário poderia definir quantos itens seriam armazenados ou exibidos na lista — no Klipper, acredito que o padrão seja algo em torno de 25 itens.

Existem outras funcionalidades do Klipper das quais talvez eu não me lembre agora, então você poderia pesquisar e complementar com recursos importantes que ajudem a deixar o comportamento do aplicativo o mais próximo possível do original. Em essência, eu gostaria de algo muito semelhante ao Klipper, quase como um clone perfeito, mas pensado para funcionar bem no Ubuntu e em outras distribuições Linux.

Pode dar um nome ao projeto.
