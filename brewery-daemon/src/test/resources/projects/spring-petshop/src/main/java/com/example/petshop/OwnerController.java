package com.example.petshop;

import java.util.List;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/owners")
public class OwnerController {
    private final OwnerService service;

    public OwnerController(OwnerService service) {
        this.service = service;
    }

    @GetMapping
    public List<String> list() {
        return service.names();
    }

    @GetMapping("/{id}")
    public String get(@PathVariable long id) {
        return service.name(id);
    }

    @PostMapping
    public String create(String name) {
        return service.add(name);
    }

    @RequestMapping("/legacy")
    public String legacy() {
        return "legacy";
    }
}
